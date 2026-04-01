use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::{Color, Modifier}};
use std::time::{Duration, Instant};

use super::*;
use crate::dispatch;
use crate::models::{Epic, EpicId, SubStatus, TaskId, TaskStatus, DEFAULT_QUICK_TASK_TITLE};

const TEST_TIMEOUT: Duration = Duration::from_secs(300);

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
fn render_to_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
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
        sub_status: SubStatus::default_for(status),
        pr_url: None,
        tag: None,
        sort_order: None,
        created_at: now,
        updated_at: now,
    }
}

fn make_app() -> App {
    App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
        make_task(3, TaskStatus::Running),
        make_task(4, TaskStatus::Done),
    ], TEST_TIMEOUT)
}

#[test]
fn tasks_by_status_filters() {
    let app = make_app();
    let backlog = app.tasks_by_status(TaskStatus::Backlog);
    assert_eq!(backlog.len(), 2);
    assert_eq!(backlog[0].id, TaskId(1));
    assert_eq!(backlog[1].id, TaskId(2));

    let running = app.tasks_by_status(TaskStatus::Running);
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, TaskId(3));

    let review = app.tasks_by_status(TaskStatus::Review);
    assert_eq!(review.len(), 0);
}

#[test]
fn move_task_forward() {
    let mut app = make_app();
    // Task 1 is in Backlog; move it forward -> Running
    let cmds = app.update(Message::MoveTask {
        id: TaskId(1),
        direction: MoveDirection::Forward,
    });
    assert_eq!(app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap().status, TaskStatus::Running);
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
fn dispatch_only_backlog_tasks() {
    let mut app = make_app();

    // Task 1 is Backlog — should dispatch
    let cmds = app.update(Message::DispatchTask(TaskId(1)));
    assert!(matches!(cmds[0], Command::Dispatch { .. }));

    // Task 3 is Running — should not dispatch
    let cmds = app.update(Message::DispatchTask(TaskId(3)));
    assert!(cmds.is_empty());

    // Task 4 is Done — should not dispatch
    let cmds = app.update(Message::DispatchTask(TaskId(4)));
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

    app.selection_mut().set_column(TaskStatus::COLUMN_COUNT - 1);
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selection().column(), TaskStatus::COLUMN_COUNT - 1); // can't go above max
}

#[test]
fn navigate_column_moves_through_visual_columns() {
    let mut app = make_app();
    assert_eq!(app.selected_column(), 0); // Backlog
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), 1); // Active
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), 2); // Blocked
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), 3); // Stale
}

#[test]
fn navigate_column_clamps_at_visual_column_max() {
    let mut app = make_app();
    app.selection_mut().set_column(TaskStatus::COLUMN_COUNT - 1); // Done column (3)
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), TaskStatus::COLUMN_COUNT - 1); // stays at 3
}

#[test]
fn navigate_row_clamps() {
    let mut app = make_app();
    // Backlog has 2 tasks (id 1, 2). Selected row starts at 0.
    app.selection_mut().set_column(0);
    app.update(Message::NavigateRow(-1));
    // Navigating up from row 0 now moves to the select-all toggle
    assert!(app.on_select_all());

    // Navigate back down to tasks and then past the end
    app.update(Message::NavigateRow(1));
    assert!(!app.on_select_all());
    app.update(Message::NavigateRow(10));
    assert_eq!(app.selection().row(0), 1); // clamps to last item index
}

#[test]
fn tick_produces_capture_for_running_tasks_with_window() {
    let mut task4 = make_task(4, TaskStatus::Running);
    task4.tmux_window = Some("main:task-4".to_string());
    let mut app = App::new(vec![task4], TEST_TIMEOUT);
    let cmds = app.update(Message::Tick);
    // Should have CaptureTmux + FetchReviewPrs + RefreshFromDb
    assert_eq!(cmds.len(), 3);
    assert!(matches!(&cmds[0], Command::CaptureTmux { id: TaskId(4), window } if window == "main:task-4"));
    assert!(matches!(&cmds[1], Command::FetchReviewPrs));
    assert!(matches!(&cmds[2], Command::RefreshFromDb));
}

#[test]
fn tick_captures_review_task_with_live_window() {
    let mut task = make_task(5, TaskStatus::Review);
    task.tmux_window = Some("task-5".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);

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
        sub_status: SubStatus::None,
        pr_url: None,
        tag: None,
        sort_order: None,
        created_at: now,
        updated_at: now,
    };
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.update(Message::Error("Something went wrong".to_string()));
    assert_eq!(app.error_popup.as_deref(), Some("Something went wrong"));
}

#[test]
fn dispatch_from_running_is_noop() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let cmds = app.update(Message::DispatchTask(TaskId(4)));
    assert!(cmds.is_empty());
}

#[test]
fn dispatch_from_review_is_noop() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = Some("task-5".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let cmds = app.update(Message::DispatchTask(TaskId(5)));
    assert!(cmds.is_empty());
}

#[test]
fn move_backward_from_running_detaches_but_keeps_worktree() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);

    let cmds = app.update(Message::MoveTask {
        id: TaskId(4),
        direction: MoveDirection::Backward,
    });

    // Should emit KillTmuxWindow then PersistTask (no Cleanup)
    assert_eq!(cmds.len(), 2);
    assert!(matches!(&cmds[0], Command::KillTmuxWindow { window } if window == "task-4"));
    assert!(matches!(&cmds[1], Command::PersistTask(_)));

    // Worktree preserved, tmux_window cleared
    let task = app.tasks.iter().find(|t| t.id == TaskId(4)).unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/4-task-4"));
    assert!(task.tmux_window.is_none());
}

#[test]
fn move_backward_from_running_without_dispatch_fields() {
    let task = make_task(3, TaskStatus::Running);
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let cmds = app.update(Message::MoveTask {
        id: TaskId(3),
        direction: MoveDirection::Backward,
    });
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], Command::PersistTask(_)));
}

#[test]
fn repo_path_empty_uses_saved_path() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/saved/repo".to_string()];

    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string(), ..Default::default() });
    app.input.buffer.clear();

    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { ref draft, .. } if draft.repo_path == "/saved/repo")));
}

#[test]
fn repo_path_empty_no_saved_stays_in_mode() {

    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/saved/repo".to_string()];

    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string(), ..Default::default() });
    app.input.buffer = "/custom/path".to_string();

    let cmds = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { ref draft, .. } if draft.repo_path == "/custom/path")));
    assert_eq!(app.tasks.len(), 0); // task not added until TaskCreated
}

#[test]
fn task_edited_updates_fields() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.update(Message::TaskEdited(TaskEdit {
        id: TaskId(1),
        title: "New".into(),
        description: "Desc".into(),
        repo_path: "/new".into(),
        status: TaskStatus::Running,
        plan: Some("docs/plan.md".into()),
        tag: None,
    }));
    assert_eq!(app.tasks[0].title, "New");
    assert_eq!(app.tasks[0].description, "Desc");
    assert_eq!(app.tasks[0].repo_path, "/new");
    assert_eq!(app.tasks[0].status, TaskStatus::Running);
    assert_eq!(app.tasks[0].plan.as_deref(), Some("docs/plan.md"));
}

#[test]
fn repo_paths_updated_replaces_paths() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.update(Message::RepoPathsUpdated(vec!["/a".into(), "/b".into()]));
    assert_eq!(app.repo_paths, vec!["/a", "/b"]);
}

#[test]
fn move_forward_to_done_enters_confirm_mode() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = None; // session closed, but worktree remains
    let mut app = App::new(vec![task], TEST_TIMEOUT);

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
    let mut app = App::new(vec![task], TEST_TIMEOUT);

    let cmds = app.update(Message::MoveTask {
        id: TaskId(5),
        direction: MoveDirection::Forward,
    });

    // Should enter confirmation mode, not move immediately
    assert!(cmds.is_empty());
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(5))));
}

#[test]
fn d_key_on_backlog_with_plan_dispatches() {
    let mut task = make_task(3, TaskStatus::Backlog);
    task.plan = Some("plan.md".into());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.selection_mut().set_column(0); // Backlog column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(&cmds[0], Command::Dispatch { .. }));
}

#[test]
fn d_key_on_running_with_window_shows_warning() {

    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("task-4".to_string());
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.selection_mut().set_column(1); // Running column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("already running"));
}

#[test]
fn d_key_on_running_no_window_resumes() {

    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.selection_mut().set_column(1); // Running column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(&cmds[0], Command::Resume { .. }));
}

#[test]
fn d_key_on_backlog_brainstorms() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.tag = Some("epic".to_string()); // tag=epic triggers brainstorm
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.selection_mut().set_column(0); // Backlog column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::Brainstorm { task } if task.id == TaskId(1)));
}

#[test]
fn d_key_on_done_shows_warning() {

    let mut app = App::new(vec![make_task(1, TaskStatus::Done)], TEST_TIMEOUT);
    app.selection_mut().set_column(3); // Done column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app.status_message.is_some());
}

#[test]
fn d_key_on_running_no_worktree_no_window_shows_warning() {

    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = None;
    task.tmux_window = None;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.selection_mut().set_column(1); // Running column
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
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.selection_mut().set_column(1); // Running column
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

    // Task 3 is Running — should not brainstorm
    let cmds = app.update(Message::BrainstormTask(TaskId(3)));
    assert!(cmds.is_empty());

    // Task 4 is Done — should not brainstorm
    let cmds = app.update(Message::BrainstormTask(TaskId(4)));
    assert!(cmds.is_empty());
}

#[test]
fn g_key_without_window_shows_message() {

    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTitle;
    app.handle_key(make_key(KeyCode::Char('H')));
    app.handle_key(make_key(KeyCode::Char('i')));
    assert_eq!(app.input.buffer, "Hi");
}

#[test]
fn backspace_pops_from_input_buffer() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTitle;
    app.input.buffer = "abc".to_string();
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "ab");
}

#[test]
fn backspace_on_empty_buffer_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTitle;
    app.input.buffer.clear();
    app.handle_key(make_key(KeyCode::Backspace));
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.mode, InputMode::InputTitle);
}

#[test]
fn enter_with_title_advances_to_tag() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTitle;
    app.input.buffer = "My Task".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputTag);
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.task_draft.as_ref().unwrap().title, "My Task");
    assert_eq!(app.status_message.as_deref(), Some("Tag: (b)ug (f)eature (c)hore (e)pic (Enter=none)"));
}

#[test]
fn enter_with_empty_title_cancels() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTitle;
    app.input.buffer.clear();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
    assert!(app.status_message.is_none());
}

#[test]
fn enter_with_whitespace_only_title_cancels() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTitle;
    app.input.buffer = "   ".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
}

#[test]
fn enter_in_description_advances_to_repo_path() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input.buffer = "/my".to_string();
    app.repo_paths = vec!["/repo1".to_string()];
    app.handle_key(make_key(KeyCode::Char('1')));
    assert_eq!(app.input.buffer, "/my1");
}

#[test]
fn zero_key_in_repo_path_appends_to_buffer() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input.buffer.clear();
    app.repo_paths = vec!["/repo".to_string()];
    app.handle_key(make_key(KeyCode::Char('0')));
    assert_eq!(app.input.buffer, "0");
}

#[test]
fn escape_from_title_mode_cancels() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    assert_eq!(app.tasks.len(), 4);
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
    assert_eq!(app.tasks.len(), 4);
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
    app.selection_mut().set_column(2); // Review column is empty
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo".to_string()];
    let cmds = app.handle_key(make_shift_key(KeyCode::Char('D')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::QuickDispatch { ref draft, epic_id: None } if draft.repo_path == "/repo"));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn shift_d_with_no_repos_shows_error() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.repo_paths = vec![];
    let cmds = app.handle_key(make_shift_key(KeyCode::Char('D')));
    assert!(cmds.is_empty());
    assert!(app.status_message.is_some());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn shift_d_with_multiple_repos_enters_quick_dispatch_mode() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = app.handle_key(make_shift_key(KeyCode::Char('D')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_mode_number_selects_repo() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::QuickDispatch { ref draft, epic_id: None } if draft.repo_path == "/repo2"));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn quick_dispatch_mode_esc_cancels() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn quick_dispatch_mode_invalid_number_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo1".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('3')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_message_emits_command() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let cmds = app.update(Message::QuickDispatch { repo_path: "/my/repo".to_string(), epic_id: None });
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::QuickDispatch { ref draft, epic_id: None }
        if draft.title == DEFAULT_QUICK_TASK_TITLE && draft.repo_path == "/my/repo")
    );
}

#[test]
fn shift_d_in_epic_view_quick_dispatches_subtask() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.repo_path = "/epic/repo".to_string();
    app.epics = vec![epic];
    app.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new(),
        saved_board: BoardSelection::new(),
    };
    let cmds = app.handle_key(make_shift_key(KeyCode::Char('D')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0],
        Command::QuickDispatch { ref draft, epic_id: Some(EpicId(10)) }
        if draft.repo_path == "/epic/repo"
    ));
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.agents.last_output_change.insert(TaskId(4), Instant::now() - Duration::from_secs(301));

    let cmds = app.update(Message::Tick);
    assert!(app.is_stale(TaskId(4)));
    assert!(cmds.iter().any(|c| matches!(c, Command::CaptureTmux { id: TaskId(4), .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.id == TaskId(4))));
}

#[test]
fn window_gone_on_running_task_marks_crashed() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());

    let cmds = app.update(Message::WindowGone(TaskId(4)));
    assert!(app.is_crashed(TaskId(4)));
    // tmux_window should NOT be cleared for crashed Running tasks
    assert!(app.tasks[0].tmux_window.is_some());
    // Should emit PersistTask to persist the Crashed sub_status
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.id == TaskId(4))));
}

#[test]
fn window_gone_on_review_task_clears_window() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Review),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());

    let cmds = app.update(Message::WindowGone(TaskId(4)));
    assert!(!app.is_crashed(TaskId(4)));
    assert!(app.tasks[0].tmux_window.is_none());
    assert!(matches!(&cmds[0], Command::PersistTask(_)));
}

#[test]
fn tmux_output_change_resets_staleness_timer() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.agents.last_output_change.insert(TaskId(4), Instant::now() - Duration::from_secs(301));
    app.agents.last_activity.insert(TaskId(4), 1000);

    app.update(Message::TmuxOutput { id: TaskId(4), output: "output".to_string(), activity_ts: 1001 });
    let elapsed = app.agents.last_output_change[&TaskId(4)].elapsed();
    assert!(elapsed < Duration::from_secs(1));
}

#[test]
fn tmux_output_same_activity_does_not_reset_timer() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    let old_instant = Instant::now() - Duration::from_secs(200);
    app.agents.last_output_change.insert(TaskId(4), old_instant);
    app.agents.last_activity.insert(TaskId(4), 1000);

    app.update(Message::TmuxOutput { id: TaskId(4), output: "output".to_string(), activity_ts: 1000 });
    let elapsed = app.agents.last_output_change[&TaskId(4)].elapsed();
    assert!(elapsed >= Duration::from_secs(199));
}

#[test]
fn activity_ts_change_with_same_output_resets_timer() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.agents.last_output_change.insert(TaskId(4), Instant::now() - Duration::from_secs(301));
    app.agents.last_activity.insert(TaskId(4), 1000);
    app.agents.tmux_outputs.insert(TaskId(4), "same output".to_string());

    // Same display text, but tmux reports new activity
    app.update(Message::TmuxOutput { id: TaskId(4), output: "same output".to_string(), activity_ts: 1001 });
    let elapsed = app.agents.last_output_change[&TaskId(4)].elapsed();
    assert!(elapsed < Duration::from_secs(1));
}

#[test]
fn activity_ts_same_with_different_output_no_reset() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    let old_instant = Instant::now() - Duration::from_secs(200);
    app.agents.last_output_change.insert(TaskId(4), old_instant);
    app.agents.last_activity.insert(TaskId(4), 1000);
    app.agents.tmux_outputs.insert(TaskId(4), "old text".to_string());

    // Different display text, but same activity timestamp
    app.update(Message::TmuxOutput { id: TaskId(4), output: "new text".to_string(), activity_ts: 1000 });
    let elapsed = app.agents.last_output_change[&TaskId(4)].elapsed();
    assert!(elapsed >= Duration::from_secs(199));
    // Display output is still updated for rendering
    assert_eq!(app.agents.tmux_outputs.get(&TaskId(4)).unwrap(), "new text");
}

#[test]
fn enter_key_toggles_detail() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    assert!(!app.detail_visible);
    app.handle_key(make_key(KeyCode::Enter));
    assert!(app.detail_visible);
}

// --- Async message handlers ---

#[test]
fn dispatched_sets_fields_and_transitions_to_running() {
    let mut task = make_task(3, TaskStatus::Backlog);
    task.plan = Some("plan.md".into());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
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
    let mut task = make_task(3, TaskStatus::Backlog);
    task.plan = Some("plan.md".into());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
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
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    let cmds = app.update(Message::Dispatched {
        id: TaskId(999),
        worktree: "/wt".to_string(),
        tmux_window: "win".to_string(),
        switch_focus: false,
    });
    assert!(cmds.is_empty());
    assert_eq!(app.tasks[0].status, TaskStatus::Backlog);
}

#[test]
fn resumed_sets_tmux_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/wt".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
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
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)], TEST_TIMEOUT);
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
    let mut app = App::new(vec![task], TEST_TIMEOUT);

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
    let mut app = App::new(vec![make_task(1, TaskStatus::Running)], TEST_TIMEOUT);
    let cmds = app.update(Message::TmuxOutput {
        id: TaskId(1),
        output: "hello".to_string(),
        activity_ts: 1000,
    });
    assert_eq!(app.agents.tmux_outputs.get(&TaskId(1)).unwrap(), "hello");
    assert!(cmds.is_empty());
}

#[test]
fn tmux_output_overwrites_previous() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Running)], TEST_TIMEOUT);
    app.update(Message::TmuxOutput { id: TaskId(1), output: "first".to_string(), activity_ts: 1000 });
    app.update(Message::TmuxOutput { id: TaskId(1), output: "second".to_string(), activity_ts: 1001 });
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
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.selection_mut().set_column(2); // Review column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("already running"));
}

#[test]
fn d_key_on_review_no_window_with_worktree_resumes() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.selection_mut().set_column(2); // Review column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(&cmds[0], Command::Resume { .. }));
}

#[test]
fn d_key_on_review_no_worktree_no_window_shows_warning() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = None;
    task.tmux_window = None;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.selection_mut().set_column(2); // Review column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("No worktree"));
}

// --- Actions on empty columns ---

#[test]
fn d_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
}

#[test]
fn g_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(cmds.is_empty());
}

#[test]
fn m_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert!(cmds.is_empty());
}

#[test]
fn shift_m_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('M')));
    assert!(cmds.is_empty());
}

#[test]
fn e_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
fn action_hints_backlog_task_with_plan() {
    let mut task = make_task(3, TaskStatus::Backlog);
    task.plan = Some("plan.md".into());
    let hints = ui::action_hints(Some(&task), Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"d"), "should have dispatch hint");
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("dispatch"), "backlog with plan dispatch means dispatch");
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

// --- epic_action_hints ---

#[test]
fn epic_action_hints_not_done() {
    let epic = make_epic(1);
    let hints = ui::epic_action_hints(&epic, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"Enter"), "epic shows detail");
    assert!(keys.contains(&"m"), "epic shows status forward");
    assert!(keys.contains(&"M"), "epic shows status backward");
    assert!(keys.contains(&"x"), "epic shows archive");
    assert!(keys.contains(&"q"), "epic shows quit");
}

#[test]
fn epic_action_hints_done() {
    let mut epic = make_epic(1);
    epic.status = TaskStatus::Done;
    let hints = ui::epic_action_hints(&epic, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"m"), "done epic shows status forward");
    assert!(keys.contains(&"M"), "done epic shows status backward");
}

// --- Edit key ---

#[test]
fn e_key_emits_edit_task_in_editor() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::EditTaskInEditor(t) if t.id == TaskId(1)));
}

#[test]
fn new_app_has_empty_agent_tracking() {
    let app = App::new(vec![], TEST_TIMEOUT);
    // stale/crashed state is now on the task's sub_status field, not in AgentTracking
    assert!(app.agents.last_activity.is_empty());
}

#[test]
fn kill_and_retry_enters_confirm_mode() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].sub_status = SubStatus::Stale;

    app.update(Message::KillAndRetry(TaskId(4)));
    assert!(matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn retry_resume_emits_kill_and_resume() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.tasks[0].sub_status = SubStatus::Stale;
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.update(Message::RetryResume(TaskId(4)));

    // After retry resume, sub_status is no longer stale/crashed
    assert!(!app.is_stale(TaskId(4)));
    assert!(!app.is_crashed(TaskId(4)));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::KillTmuxWindow { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::Resume { .. })));
}

#[test]
fn retry_fresh_emits_cleanup_and_dispatch() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.tasks[0].sub_status = SubStatus::Stale;
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.update(Message::RetryFresh(TaskId(4)));

    assert!(!app.is_stale(TaskId(4)));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert_eq!(app.tasks[0].status, TaskStatus::Backlog);
    assert!(cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::Dispatch { .. })));
}

#[test]
fn d_key_on_stale_running_task_enters_retry_mode() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].sub_status = SubStatus::Stale;
    // Navigate to Running column (index 1)
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn d_key_on_crashed_running_task_enters_retry_mode() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].sub_status = SubStatus::Crashed;
    // Navigate to Running column (index 1)
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn confirm_retry_r_key_emits_resume() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty());
}

// --- Message-level tests for new input routing handlers ---

#[test]
fn dismiss_error_clears_popup() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
fn submit_title_with_text_advances_to_tag() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTitle;
    app.update(Message::SubmitTitle("My Task".to_string()));
    assert_eq!(app.input.mode, InputMode::InputTag);
    assert_eq!(app.input.task_draft.as_ref().unwrap().title, "My Task");
    assert_eq!(app.status_message.as_deref(), Some("Tag: (b)ug (f)eature (c)hore (e)pic (Enter=none)"));
}

#[test]
fn submit_empty_title_cancels() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTitle;
    app.update(Message::SubmitTitle(String::new()));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
}

#[test]
fn submit_tag_advances_to_description() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), ..Default::default() });
    let cmds = app.update(Message::SubmitTag(Some("bug".to_string())));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::InputDescription);
    assert_eq!(app.input.task_draft.as_ref().unwrap().tag, Some("bug".to_string()));
    assert_eq!(app.status_message.as_deref(), Some("Enter description: "));
}

#[test]
fn submit_description_advances_to_repo_path() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), ..Default::default() });
    app.update(Message::SubmitDescription("my desc".to_string()));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert_eq!(app.input.task_draft.as_ref().unwrap().description, "my desc");
}

#[test]
fn submit_repo_path_creates_task() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: "D".to_string(), tag: Some("bug".to_string()), ..Default::default() });
    let cmds = app.update(Message::SubmitRepoPath("/my/repo".to_string()));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { ref draft, .. } if draft.repo_path == "/my/repo" && draft.tag == Some("bug".to_string()))));
}

#[test]
fn input_char_appends_to_buffer() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputTitle;
    app.update(Message::InputChar('H'));
    app.update(Message::InputChar('i'));
    assert_eq!(app.input.buffer, "Hi");
}

#[test]
fn start_repo_filter_enters_mode() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.update(Message::StartRepoFilter);
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn toggle_repo_filter_adds_and_removes() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.update(Message::ToggleRepoFilter("/repo-a".to_string()));
    assert!(app.repo_filter.contains("/repo-a"));
    assert!(!app.repo_filter.contains("/repo-b"));

    app.update(Message::ToggleRepoFilter("/repo-a".to_string()));
    assert!(!app.repo_filter.contains("/repo-a"));
}

#[test]
fn toggle_all_repo_filter_selects_all_then_clears() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;

    // Toggle all on
    app.update(Message::ToggleAllRepoFilter);
    assert_eq!(app.repo_filter.len(), 2);

    // Toggle all off
    app.update(Message::ToggleAllRepoFilter);
    assert!(app.repo_filter.is_empty());
}

#[test]
fn close_repo_filter_returns_to_normal() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    let cmds = app.update(Message::CloseRepoFilter);
    assert_eq!(app.input.mode, InputMode::Normal);
    // Should emit PersistStringSetting
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistStringSetting { .. })));
}

#[test]
fn input_backspace_removes_last_char() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::ConfirmDelete;
    app.status_message = Some("Delete \"Task 1\" [backlog]? (y/n)".to_string());
    app.update(Message::CancelDelete);
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
}

#[test]
fn status_info_sets_message() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.update(Message::StatusInfo("hello".to_string()));
    assert_eq!(app.status_message.as_deref(), Some("hello"));
}

#[test]
fn start_quick_dispatch_selection_enters_mode() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.update(Message::StartQuickDispatchSelection);
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
    assert!(app.status_message.is_some());
}

#[test]
fn select_quick_dispatch_repo_dispatches() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = app.update(Message::SelectQuickDispatchRepo(1));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::QuickDispatch { ref draft, .. } if draft.repo_path == "/repo2")));
}

#[test]
fn select_quick_dispatch_repo_out_of_range_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo1".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.update(Message::SelectQuickDispatchRepo(5));
    assert!(cmds.is_empty());
    // Mode is not changed by the handler (stays as-is)
}

#[test]
fn cancel_retry_returns_to_normal() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
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
    let mut app = App::new(vec![task], TEST_TIMEOUT);

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
    ], TEST_TIMEOUT);
    let cmds = app.update(Message::ArchiveTask(TaskId(1)));
    assert!(!cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn archive_clears_agent_tracking() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("dev:1-test".to_string());
    task.sub_status = SubStatus::Stale;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.agents.tmux_outputs.insert(TaskId(1), "output".to_string());
    app.agents.last_activity.insert(TaskId(1), 1000);

    app.update(Message::ArchiveTask(TaskId(1)));

    // stale/crashed state is now on the task's sub_status field
    assert!(!app.agents.tmux_outputs.contains_key(&TaskId(1)));
    assert!(!app.agents.last_activity.contains_key(&TaskId(1)));
}

// --- Archive panel key handling ---

#[test]
fn archive_panel_j_k_navigation() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
        make_task(3, TaskStatus::Archived),
    ], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
    app.archive.visible = true;

    app.handle_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT));
    assert!(!app.archive.visible);
}

#[test]
fn archive_panel_x_enters_confirm_delete() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);

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
    let mut app = App::new(vec![task, make_task(2, TaskStatus::Backlog)], TEST_TIMEOUT);

    // Navigate to Running column (column 1)
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
    app.update(Message::NavigateColumn(2));
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

    // Both should now be Running
    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Running);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Running);
    // Should have PersistTask commands
    let persist_count = cmds.iter().filter(|c| matches!(c, Command::PersistTask(_))).count();
    assert_eq!(persist_count, 2);
}

#[test]
fn batch_move_clears_selection() {
    let mut app = make_app();
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    app.handle_key(make_key(KeyCode::Char('m')));

    assert!(app.selected_tasks.is_empty());
}

#[test]
fn batch_move_multiple_steps() {
    let mut app = make_app();
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    // Move Backlog -> Running (clears selection)
    app.handle_key(make_key(KeyCode::Char('m')));

    // Re-select and move Running -> Review
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));
    app.handle_key(make_key(KeyCode::Char('m')));

    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Review);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Review);
}

#[test]
fn batch_move_backward() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
        make_task(3, TaskStatus::Done),
    ], TEST_TIMEOUT);

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
        make_task(3, TaskStatus::Backlog),
    ], TEST_TIMEOUT);

    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    let cmds = app.update(Message::BatchArchiveTasks(vec![TaskId(1), TaskId(2)]));

    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Archived);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Archived);
    assert_eq!(app.find_task(TaskId(3)).unwrap().status, TaskStatus::Backlog);
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
    assert_eq!(app.status_message.as_deref(), Some("Archive 2 items? (y/n)"));
}

#[test]
fn confirm_archive_with_selection_dispatches_batch() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
    ], TEST_TIMEOUT);

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
    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Running);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "backlog"));
    assert!(buffer_contains(&buf, "running"));
    assert!(buffer_contains(&buf, "review"));
    assert!(buffer_contains(&buf, "done"));
}

#[test]
fn render_shows_task_titles_in_columns() {
    let tasks = vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
        make_task(3, TaskStatus::Review),
    ];
    let mut app = App::new(tasks, TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "Task 1"));
    assert!(buffer_contains(&buf, "Task 2"));
    assert!(buffer_contains(&buf, "Task 3"));
}

#[test]
fn render_error_popup_shows_message() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.update(Message::Error("Something went wrong".to_string()));
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "Something went wrong"));
}

#[test]
fn render_status_bar_shows_keybindings() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "uit"));
}

#[test]
fn render_crashed_task_shows_label() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("win-1".to_string());
    task.sub_status = SubStatus::Crashed;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "crashed"));
}

#[test]
fn render_stale_task_shows_label() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("win-1".to_string());
    task.sub_status = SubStatus::Stale;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "stale"));
}

// ---------------------------------------------------------------------------
// ui.rs — detached indicator
// ---------------------------------------------------------------------------

#[test]
fn running_card_with_worktree_no_window_shows_detached() {
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "○ detached"), "expected '○ detached'");
}

#[test]
fn running_card_with_window_shows_running_not_detached() {
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = Some("1-fix".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "◉ running"), "expected '◉ running'");
    assert!(!buffer_contains(&buf, "detached"), "should not show detached");
}

#[test]
fn crashed_card_with_no_window_shows_detached_not_crashed() {
    // Detached out-prioritizes Crashed when tmux_window is None
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Crashed;
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "○ detached"), "expected '○ detached'");
    assert!(!buffer_contains(&buf, "\u{26a0} crashed"), "should not show ⚠ crashed");
}

#[test]
fn review_card_with_pr_detached_shows_circle_prefix() {
    let mut task = make_task(1, TaskStatus::Review);
    task.sub_status = SubStatus::AwaitingReview;
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "○ PR #42"), "expected '○ PR #42'");
}

#[test]
fn review_card_with_pr_not_detached_shows_no_prefix() {
    let mut task = make_task(1, TaskStatus::Review);
    task.sub_status = SubStatus::AwaitingReview;
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = Some("1-fix".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "PR #42"), "expected 'PR #42'");
    assert!(!buffer_contains(&buf, "○ PR"), "should not show circle prefix");
}

#[test]
fn render_does_not_panic_on_small_terminal() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    // Very small terminal — should not panic
    let _ = render_to_buffer(&mut app, 20, 5);
}

#[test]
fn render_input_mode_shows_prompt() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.update(Message::StartNewTask);
    let buf = render_to_buffer(&mut app, 100, 20);
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
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    // Cursor card uses thicker stripe ▌ (U+258C), non-cursor uses ▎ (U+258E)
    assert!(
        buffer_contains(&buf, "\u{258c}") || buffer_contains(&buf, "\u{258e}"),
        "task card should have stripe character"
    );
}

#[test]
fn render_v2_backlog_task_shows_status_icon() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "\u{25e6}"), "backlog task should show \u{25e6} icon");
}

#[test]
fn render_v2_running_task_shows_status_icon() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("win-1".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "\u{25c9}"), "running task should show \u{25c9} icon");
}

#[test]
fn render_v2_focused_column_shows_arrow() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    // Default focus is on first column (Backlog), should show \u{25b8}
    assert!(buffer_contains(&buf, "\u{25b8}"), "focused column should show \u{25b8} indicator");
}

#[test]
fn render_v2_unfocused_columns_show_dot() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
    // Unfocused columns should show \u{25e6}
    assert!(buffer_contains(&buf, "\u{25e6}"), "unfocused columns should show \u{25e6} indicator");
}

#[test]
fn render_v2_detail_panel_shows_inline_metadata() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.update(Message::ToggleDetail);
    let buf = render_to_buffer(&mut app, 120, 20);
    // The compact detail panel shows "title \u{00b7} #id \u{00b7} status \u{00b7} repo" on one line
    // Check for the middle-dot separator which is new in v2
    assert!(buffer_contains(&buf, "\u{00b7}"), "detail panel should use \u{00b7} separator");
    assert!(buffer_contains(&buf, "#1"), "detail panel should show task ID with # prefix");
}

#[test]
fn render_v2_status_bar_no_brackets() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 20);
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
    let mut app = App::new(vec![make_task(1, TaskStatus::Done)], TEST_TIMEOUT);
    // Navigate to Done column (index 3)
    for _ in 0..3 {
        app.update(Message::NavigateColumn(1));
    }
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "\u{2713}"), "done task should show \u{2713} icon");
}

// ---------------------------------------------------------------------------
// Rendering tests — layout correctness
// ---------------------------------------------------------------------------

#[test]
fn render_columns_appear_left_to_right() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 120, 30);

    // Find the leftmost x-position where each header appears
    let headers = ["backlog", "running", "review", "done"];
    let mut positions: Vec<Option<u16>> = Vec::new();
    for header in &headers {
        let mut found = None;
        for y in 0..2u16 {
            for x in 0..120u16 {
                let remaining = (120 - x) as usize;
                if remaining < header.len() {
                    continue;
                }
                let segment: String = (0..header.len() as u16)
                    .map(|dx| buf[(x + dx, y)].symbol().to_string())
                    .collect();
                if segment == *header {
                    found = Some(x);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        positions.push(found);
    }

    // All headers must render
    for (i, header) in headers.iter().enumerate() {
        assert!(positions[i].is_some(), "column header '{header}' not found in rendered output");
    }

    // Verify strict left-to-right ordering
    let xs: Vec<u16> = positions.into_iter().flatten().collect();
    for pair in xs.windows(2) {
        assert!(pair[0] < pair[1], "columns must be ordered left to right, got positions: {xs:?}");
    }
}

#[test]
fn render_columns_fill_terminal_width() {
    // Regression test: columns must use the full terminal width, not leave a gap on the right.
    // A previous bug reserved a 34-char right sidebar in the column content area.
    let mut app = App::new(
        vec![make_task(1, TaskStatus::Done)],
        TEST_TIMEOUT,
    );
    let width: u16 = 120;
    let buf = render_to_buffer(&mut app, width, 20);

    // Find the rightmost x-position where "done" header text appears
    let header = "done";
    let mut header_x = None;
    'outer: for y in 0..3u16 {
        for x in (0..width).rev() {
            let remaining = (width - x) as usize;
            if remaining < header.len() {
                continue;
            }
            let segment: String = (0..header.len() as u16)
                .map(|dx| buf[(x + dx, y)].symbol().to_string())
                .collect();
            if segment == header {
                header_x = Some(x);
                break 'outer;
            }
        }
    }
    let done_col_x = header_x.expect("'done' column header not found");

    // The "done" column header should be centered in the last quarter of the terminal.
    // With 4 columns at width=120, each column is 30 chars wide, so the last column
    // starts at x=90. The header should be somewhere after x=90.
    // If the old bug exists (34-char sidebar), each column is only ~21 chars and the
    // header would be well before x=90.
    let expected_min_x = (width * 3 / 4) as u16;
    assert!(
        done_col_x >= expected_min_x,
        "last column header 'done' at x={done_col_x}, expected >= {expected_min_x} — \
         columns are not filling the terminal width"
    );
}

#[test]
fn render_help_overlay_shows_keybindings_help() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.update(Message::ToggleHelp);
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(buffer_contains(&buf, "Navigation"), "help overlay should show Navigation section");
    assert!(buffer_contains(&buf, "Actions"), "help overlay should show Actions section");
}

#[test]
fn render_help_overlay_in_review_board_shows_review_shortcuts() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.update(Message::SwitchToReviewBoard);
    app.update(Message::ToggleHelp);
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        buffer_contains(&buf, "Review Board"),
        "review help should have Review Board section"
    );
    assert!(
        buffer_contains(&buf, "open PR"),
        "review help should mention open PR"
    );
    assert!(
        buffer_contains(&buf, "dispatch review agent"),
        "review help should mention dispatch review agent"
    );
    assert!(
        !buffer_contains(&buf, "new task"),
        "review help should not show task board new task key"
    );
}

#[test]
fn render_1x1_terminal_does_not_panic() {
    let mut app = App::new(
        vec![make_task(1, TaskStatus::Running)],
        TEST_TIMEOUT,
    );
    let _ = render_to_buffer(&mut app, 1, 1);
}

#[test]
fn render_archive_overlay_shows_archived_tasks() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.status = TaskStatus::Archived;
    task.title = "Archived Item".to_string();
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.update(Message::ToggleArchive);
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(buffer_contains(&buf, "Archived Item"), "archive overlay should show archived task title");
}

// ---------------------------------------------------------------------------
// Stress tests
// ---------------------------------------------------------------------------

#[test]
fn stress_large_task_list_navigation() {
    let tasks: Vec<_> = (1..=1000).map(|i| make_task(i, TaskStatus::Backlog)).collect();
    let mut app = App::new(tasks, TEST_TIMEOUT);

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
        task.status = match i % 4 {
            0 => TaskStatus::Backlog,
            1 => TaskStatus::Running,
            2 => TaskStatus::Review,
            _ => TaskStatus::Done,
        };
    }
    let mut app = App::new(tasks, TEST_TIMEOUT);

    // Render at various sizes — must not panic
    for width in [40, 80, 120, 200] {
        for height in [10, 24, 50] {
            let _ = render_to_buffer(&mut app, width, height);
        }
    }
}

#[test]
fn stress_rapid_status_transitions() {
    let tasks = vec![make_task(1, TaskStatus::Backlog)];
    let mut app = App::new(tasks, TEST_TIMEOUT);

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
    let mut app = App::new(tasks, TEST_TIMEOUT);
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
        repo_path: "/repo".to_string(),
        status: TaskStatus::Backlog,
        plan: None,
        sort_order: None,
        created_at: now,
        updated_at: now,
    }
}

// --- tasks_for_current_view ---

#[test]
fn tasks_for_current_view_board_excludes_epic_tasks() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let standalone = make_task(1, TaskStatus::Backlog);
    let mut subtask = make_task(2, TaskStatus::Running);
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
fn enter_on_epic_toggles_detail() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    // Epic is at row 0 in Backlog column (no standalone tasks)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    assert!(!app.detail_visible);
    app.handle_key(make_key(KeyCode::Enter));
    assert!(app.detail_visible, "Enter on epic should toggle detail panel");
    assert!(matches!(app.view_mode, ViewMode::Board(_)), "Should stay in board view");
}

#[test]
fn e_on_epic_enters_epic_view() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    app.handle_key(make_key(KeyCode::Char('e')));

    match &app.view_mode {
        ViewMode::Epic { epic_id, .. } => assert_eq!(*epic_id, EpicId(10)),
        _ => panic!("Expected ViewMode::Epic after pressing 'e' on epic"),
    }
}

#[test]
fn enter_on_task_still_toggles_detail() {
    let mut app = make_app();
    assert!(!app.detail_visible);
    app.handle_key(make_key(KeyCode::Enter));
    assert!(app.detail_visible, "Enter on task should still toggle detail");
}

#[test]
fn e_on_task_still_edits() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(cmds.iter().any(|c| matches!(c, Command::EditTaskInEditor(_))));
}

#[test]
fn enter_epic_switches_to_epic_view() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.update(Message::ExitEpic);
    assert!(matches!(app.view_mode, ViewMode::Board(_)));
}

// --- ColumnItem ---

#[test]
fn column_items_board_view_includes_epics() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)]; // epic with no subtasks = Backlog

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert_eq!(items.len(), 2); // 1 task + 1 epic
    // Epic sorts first: Unplanned priority 0 < task SubStatus::None priority 5
    assert!(matches!(items[0], ColumnItem::Epic(_)));
    assert!(matches!(items[1], ColumnItem::Task(_)));
}

#[test]
fn column_items_epic_view_no_epics() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];

    // Epic at row 0 (Unplanned priority 0), Task at row 1 (SubStatus::None priority 5)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let epic = make_epic(1);
    app.update(Message::EpicCreated(epic));
    assert_eq!(app.epics().len(), 1);
}

#[test]
fn delete_epic_removes_from_state_and_tasks() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
fn move_epic_status_forward() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)]; // starts as Backlog
    let cmds = app.update(Message::MoveEpicStatus(EpicId(10), MoveDirection::Forward));
    assert_eq!(app.epics[0].status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistEpic { id: EpicId(10), status: Some(TaskStatus::Running), .. })));
}

#[test]
fn move_epic_status_backward() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Done;
    app.epics = vec![epic];
    let cmds = app.update(Message::MoveEpicStatus(EpicId(10), MoveDirection::Backward));
    assert_eq!(app.epics[0].status, TaskStatus::Review);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistEpic { id: EpicId(10), status: Some(TaskStatus::Review), .. })));
}

// ---------------------------------------------------------------------------
// input.rs — Normal mode: Epic interactions
// ---------------------------------------------------------------------------

/// Helper: create an app with one task + one epic in Backlog, cursor on the epic.
fn make_app_with_epic_selected() -> App {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    // Epic at row 0 (Unplanned priority 0), Task at row 1 (SubStatus::None priority 5)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);
    app
}

#[test]
fn m_key_on_epic_moves_status_forward() {
    let mut app = make_app_with_epic_selected();
    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert_eq!(app.epics[0].status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistEpic { .. })));
}

#[test]
fn shift_m_key_on_backlog_epic_stays_backlog() {
    let mut app = make_app_with_epic_selected();
    let cmds = app.handle_key(make_key(KeyCode::Char('M')));
    // Already at Backlog, can't go backward
    assert_eq!(app.epics[0].status, TaskStatus::Backlog);
    assert!(cmds.is_empty());
}

#[test]
fn shift_m_on_done_epic_moves_to_review() {
    let mut app = App::new(vec![
        {
            let mut t = make_task(1, TaskStatus::Done);
            t.epic_id = Some(EpicId(10));
            t
        },
    ], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Done;
    app.epics = vec![epic];
    // Done epic → column 3
    app.selection_mut().set_column(3);
    app.selection_mut().set_row(3, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('M')));
    assert_eq!(app.epics[0].status, TaskStatus::Review);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistEpic { id: EpicId(10), status: Some(TaskStatus::Review), .. })));
}

#[test]
fn shift_e_key_starts_new_epic() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('E')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::InputEpicTitle);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
fn x_key_on_epic_with_non_done_subtasks_rejects_archive() {
    let mut app = App::new(vec![
        {
            let mut t = make_task(1, TaskStatus::Backlog);
            t.epic_id = Some(EpicId(10));
            t
        },
        {
            let mut t = make_task(2, TaskStatus::Running);
            t.epic_id = Some(EpicId(10));
            t
        },
    ], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.epics = vec![epic];
    // Subtasks are hidden in board view. Epic status is Running (col 1).
    // Epic is the only item in Running column → row 0.
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('x')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.as_deref().unwrap().contains("Cannot archive epic"));
    assert!(app.status_message.as_deref().unwrap().contains("2 subtasks not done"));
}

#[test]
fn x_key_on_epic_with_mixed_subtasks_rejects_archive_with_count() {
    let mut app = App::new(vec![
        {
            let mut t = make_task(1, TaskStatus::Done);
            t.epic_id = Some(EpicId(10));
            t
        },
        {
            let mut t = make_task(2, TaskStatus::Done);
            t.epic_id = Some(EpicId(10));
            t
        },
        {
            let mut t = make_task(3, TaskStatus::Running);
            t.epic_id = Some(EpicId(10));
            t
        },
    ], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.epics = vec![epic];
    // 2 Done + 1 Running → epic status Running (col 1). Epic is only item → row 0.
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('x')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.as_deref().unwrap().contains("1 subtask not done"));
}

#[test]
fn x_key_on_epic_with_all_done_subtasks_allows_archive() {
    let mut app = App::new(vec![
        {
            let mut t = make_task(1, TaskStatus::Done);
            t.epic_id = Some(EpicId(10));
            t
        },
    ], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Done;
    app.epics = vec![epic];
    // All done → epic status Done (column 3). Epic is only item → row 0.
    app.selection_mut().set_column(3);
    app.selection_mut().set_row(3, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('x')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmArchiveEpic);
    assert!(app.status_message.as_deref().unwrap().contains("Archive epic"));
}

#[test]
fn confirm_archive_epic_no_subtasks_allows_archive() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    // No subtasks → derived status Backlog (col 0). Epic is only item → row 0.
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);
    let cmds = app.update(Message::ConfirmArchiveEpic);
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmArchiveEpic);
    assert!(app.status_message.as_deref().unwrap().contains("Archive epic"));
}

#[test]
fn enter_key_on_epic_enters_epic_view() {
    // After keybinding swap: 'e' enters epic view, Enter toggles detail
    let mut app = make_app_with_epic_selected();
    app.handle_key(make_key(KeyCode::Char('e')));
    assert!(matches!(app.view_mode, ViewMode::Epic { epic_id: EpicId(10), .. }));
}

#[test]
fn e_key_in_epic_view_edits_epic() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
fn e_key_on_task_in_epic_view_edits_task_not_epic() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    let mut subtask = make_task(1, TaskStatus::Backlog);
    subtask.epic_id = Some(EpicId(10));
    app.tasks = vec![subtask];
    app.update(Message::EnterEpic(EpicId(10)));

    // Cursor on the subtask in the Backlog column (col 0, row 0)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert_eq!(cmds.len(), 1, "expected exactly one command");
    assert!(
        matches!(&cmds[0], Command::EditTaskInEditor(t) if t.id == TaskId(1)),
        "expected EditTaskInEditor(task 1), got {:?}",
        cmds
    );
}

#[test]
fn esc_in_epic_view_exits_to_board() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new(),
        saved_board: BoardSelection::new(),
    };
    app.handle_key(make_key(KeyCode::Esc));
    assert!(matches!(app.view_mode, ViewMode::Board(_)));
}

#[test]
fn d_key_on_backlog_epic_dispatches_epic() {
    let mut app = make_app_with_epic_selected(); // epic at row 1 in Backlog
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], Command::DispatchEpic { ref epic } if epic.id == EpicId(10)));
}

#[test]
fn d_key_in_epic_view_with_no_subtasks_dispatches_epic() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let epic = make_epic(10);
    app.epics = vec![epic];
    app.update(Message::EnterEpic(EpicId(10)));

    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.iter().any(|c| matches!(c, Command::DispatchEpic { ref epic } if epic.id == EpicId(10))));
}

// ---------------------------------------------------------------------------
// DispatchEpic message
// ---------------------------------------------------------------------------

#[test]
fn dispatch_epic_on_backlog_epic_produces_command() {
    let mut app = make_app_with_epic_selected(); // epic at row 1, Backlog column
    let cmds = app.update(Message::DispatchEpic(EpicId(10)));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], Command::DispatchEpic { ref epic } if epic.id == EpicId(10)));
}

#[test]
fn dispatch_epic_on_non_backlog_shows_status() {
    let mut app = App::new(vec![
        {
            let mut t = make_task(1, TaskStatus::Running);
            t.epic_id = Some(EpicId(10));
            t
        },
    ], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.epics = vec![epic];

    // Epic status is Running (not Backlog) — dispatch should be rejected
    let cmds = app.update(Message::DispatchEpic(EpicId(10)));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_ref().unwrap().contains("No backlog tasks"));
}

#[test]
fn dispatch_epic_with_plan_dispatches_next_backlog_subtask() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.plan = Some("docs/plan.md".to_string());
    app.epics = vec![epic];

    // Add two backlog subtasks for this epic
    let mut task1 = make_task(1, TaskStatus::Backlog);
    task1.epic_id = Some(EpicId(10));
    task1.plan = Some("plan1.md".to_string());
    let mut task2 = make_task(2, TaskStatus::Backlog);
    task2.epic_id = Some(EpicId(10));

    app.tasks = vec![task1.clone(), task2];

    // Select the epic (only item in backlog column at row 0)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    let cmds = app.update(Message::DispatchEpic(EpicId(10)));
    // Should dispatch task1 (first backlog subtask, has plan)
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], Command::Dispatch { ref task } if task.id == TaskId(1)));
}

#[test]
fn dispatch_epic_with_plan_brainstorms_subtask_without_plan() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.plan = Some("docs/plan.md".to_string());
    app.epics = vec![epic];

    // Subtask without a plan, tagged as "epic" to trigger brainstorm
    let mut task1 = make_task(1, TaskStatus::Backlog);
    task1.epic_id = Some(EpicId(10));
    task1.tag = Some("epic".to_string());
    app.tasks = vec![task1];

    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    let cmds = app.update(Message::DispatchEpic(EpicId(10)));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], Command::Brainstorm { ref task } if task.id == TaskId(1)));
}

#[test]
fn dispatch_epic_with_plan_no_backlog_subtasks_falls_back_to_planning() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.plan = Some("docs/plan.md".to_string());
    app.epics = vec![epic];

    // Only an archived subtask — archived tasks are excluded from epic_status
    // so the epic stays Backlog, but there are no backlog subtasks to dispatch
    let mut task1 = make_task(1, TaskStatus::Archived);
    task1.epic_id = Some(EpicId(10));
    app.tasks = vec![task1];

    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    let cmds = app.update(Message::DispatchEpic(EpicId(10)));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], Command::DispatchEpic { ref epic } if epic.id == EpicId(10)));
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer = "partial".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
}

#[test]
fn epic_title_enter_with_text_advances_to_description() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer = "My Epic".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputEpicDescription);
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.epic_draft.as_ref().unwrap().title, "My Epic");
}

#[test]
fn epic_title_enter_empty_cancels() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer.clear();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn epic_description_enter_advances_to_repo_path() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputEpicRepoPath;
    app.input.epic_draft = Some(EpicDraft { title: "E".to_string(), description: "D".to_string(), ..Default::default() });
    app.input.buffer = "/my/repo".to_string();
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertEpic(ref d) if d.repo_path == "/my/repo")));
}

#[test]
fn epic_repo_path_enter_empty_uses_saved_path() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputEpicTitle;
    app.handle_key(make_key(KeyCode::Char('A')));
    app.handle_key(make_key(KeyCode::Char('b')));
    assert_eq!(app.input.buffer, "Ab");
}

#[test]
fn epic_text_input_backspace_removes() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer = "abc".to_string();
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "ab");
}

#[test]
fn epic_text_input_unrecognized_key_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer = "x".to_string();
    let cmds = app.handle_key(make_key(KeyCode::Tab));
    assert!(cmds.is_empty());
    assert_eq!(app.input.buffer, "x");
    assert_eq!(app.input.mode, InputMode::InputEpicTitle);
}

#[test]
fn epic_repo_path_digit_quick_selects() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/first".to_string(), "/second".to_string()];
    app.input.mode = InputMode::InputEpicRepoPath;
    app.input.epic_draft = Some(EpicDraft { title: "E".to_string(), description: "D".to_string(), ..Default::default() });
    app.input.buffer.clear();
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertEpic(ref d) if d.repo_path == "/second")));
}

#[test]
fn epic_repo_path_digit_with_nonempty_buffer_appends() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/first".to_string()];
    app.input.mode = InputMode::InputEpicRepoPath;
    app.input.epic_draft = Some(EpicDraft { title: "E".to_string(), description: "D".to_string(), ..Default::default() });
    app.input.buffer = "/my".to_string();
    let cmds = app.handle_key(make_key(KeyCode::Char('1')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.buffer, "/my1");
}

// ---------------------------------------------------------------------------
// input.rs — handle_key_confirm_delete_epic
// ---------------------------------------------------------------------------

fn make_app_confirm_delete_epic() -> App {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0); // cursor on epic (sorts first with Unplanned priority 0)
    app.input.mode = InputMode::ConfirmDeleteEpic;
    app.status_message = Some("Delete epic \"Epic 10\" and subtasks? (y/n)".to_string());
    app
}

#[test]
fn confirm_delete_epic_enters_mode_with_title() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0); // cursor on epic (sorts first with Unplanned priority 0)
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
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0); // cursor on epic (sorts first with Unplanned priority 0)
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
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmArchiveEpic;
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty());
}

// ---------------------------------------------------------------------------
// input.rs — g key on epic
// ---------------------------------------------------------------------------

#[test]
fn g_key_on_epic_jumps_to_review_subtask() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Review;
    app.epics = vec![epic];

    // Subtask in Review with a tmux window
    let mut subtask = make_task(1, TaskStatus::Review);
    subtask.epic_id = Some(EpicId(10));
    subtask.tmux_window = Some("win-1".to_string());
    app.tasks = vec![subtask];

    // Epic status is Review => column 2
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(matches!(&cmds[0], Command::JumpToTmux { window } if window == "win-1"));
}

#[test]
fn g_key_on_epic_no_review_session_enters_epic_view() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Review;
    app.epics = vec![epic];

    // Subtask in Review but NO tmux window
    let mut subtask = make_task(1, TaskStatus::Review);
    subtask.epic_id = Some(EpicId(10));
    app.tasks = vec![subtask];

    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);

    let _cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(matches!(app.view_mode, ViewMode::Epic { epic_id, .. } if epic_id == EpicId(10)));
}

#[test]
fn g_key_on_epic_jumps_to_blocked_running_subtask() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.epics = vec![epic];

    let mut subtask = make_task(1, TaskStatus::Running);
    subtask.epic_id = Some(EpicId(10));
    subtask.sub_status = SubStatus::NeedsInput;
    subtask.tmux_window = Some("win-blocked".to_string());
    app.tasks = vec![subtask];

    // Epic status is Running => column 1
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(matches!(&cmds[0], Command::JumpToTmux { window } if window == "win-blocked"));
}

#[test]
fn g_key_on_epic_jumps_to_stale_running_subtask() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.epics = vec![epic];

    let mut subtask = make_task(1, TaskStatus::Running);
    subtask.epic_id = Some(EpicId(10));
    subtask.sub_status = SubStatus::Stale;
    subtask.tmux_window = Some("win-stale".to_string());
    app.tasks = vec![subtask];

    // Epic status is Running => column 1
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(matches!(&cmds[0], Command::JumpToTmux { window } if window == "win-stale"));
}

#[test]
fn g_key_on_epic_no_active_window_enters_epic_view() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let epic = make_epic(10);
    app.epics = vec![epic];

    // Epic in Backlog, no subtasks with tmux windows
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    let _cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(matches!(app.view_mode, ViewMode::Epic { epic_id, .. } if epic_id == EpicId(10)));
}

#[test]
fn g_key_on_epic_with_running_subtask_jumps_to_tmux() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.epics = vec![epic];

    let mut subtask = make_task(1, TaskStatus::Running);
    subtask.epic_id = Some(EpicId(10));
    subtask.tmux_window = Some("win-running".to_string());
    app.tasks = vec![subtask];

    // Epic status is Running => column 1; subtasks hidden in board view, epic at row 0
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(matches!(&cmds[0], Command::JumpToTmux { window } if window == "win-running"));
}

#[test]
fn g_key_on_epic_prefers_running_over_review() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.epics = vec![epic];

    let mut review_task = make_task(1, TaskStatus::Review);
    review_task.epic_id = Some(EpicId(10));
    review_task.tmux_window = Some("win-review".to_string());

    let mut running_task = make_task(2, TaskStatus::Running);
    running_task.epic_id = Some(EpicId(10));
    running_task.tmux_window = Some("win-running".to_string());

    app.tasks = vec![review_task, running_task];

    // Epic status is Running => column 1; subtasks hidden in board view, epic at row 0
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(matches!(&cmds[0], Command::JumpToTmux { window } if window == "win-running"));
}

#[test]
fn g_key_on_epic_picks_lowest_sort_order() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.epics = vec![epic];

    let mut task_high = make_task(1, TaskStatus::Running);
    task_high.epic_id = Some(EpicId(10));
    task_high.sort_order = Some(5);
    task_high.tmux_window = Some("win-high".to_string());

    let mut task_low = make_task(2, TaskStatus::Running);
    task_low.epic_id = Some(EpicId(10));
    task_low.sort_order = Some(1);
    task_low.tmux_window = Some("win-low".to_string());

    app.tasks = vec![task_high, task_low];

    // Epic status is Running => column 1; subtasks hidden in board view, epic at row 0
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(matches!(&cmds[0], Command::JumpToTmux { window } if window == "win-low"));
}

// ---------------------------------------------------------------------------
// input.rs — Archive panel extras
// ---------------------------------------------------------------------------

#[test]
fn archive_panel_down_arrow_navigates() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
    ], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
    app.archive.visible = true;
    app.archive.selected_row = 1;
    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(app.archive.selected_row, 0);
}

#[test]
fn archive_panel_esc_closes() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], TEST_TIMEOUT);
    app.archive.visible = true;
    app.handle_key(make_key(KeyCode::Esc));
    assert!(!app.archive.visible);
}

#[test]
fn archive_panel_e_edits_task() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], TEST_TIMEOUT);
    app.archive.visible = true;
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::EditTaskInEditor(t) if t.id == TaskId(1)));
}

#[test]
fn archive_panel_e_on_empty_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.archive.visible = true;
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(cmds.is_empty());
}

#[test]
fn archive_panel_x_on_empty_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.archive.visible = true;
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::Normal); // did not enter ConfirmDelete
}

#[test]
fn archive_panel_q_quits() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], TEST_TIMEOUT);
    app.archive.visible = true;
    app.handle_key(make_key(KeyCode::Char('q')));
    assert!(app.should_quit);
}

#[test]
fn archive_panel_unrecognized_key_is_noop() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('0')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_non_digit_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    let mut app = App::new(vec![], TEST_TIMEOUT);
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
    ], TEST_TIMEOUT);
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

    let buf = render_to_buffer(&mut app, 80, 30);
    assert!(buffer_contains(&buf, "Navigation"));
    assert!(buffer_contains(&buf, "Actions"));
    assert!(buffer_contains(&buf, "General"));
}

#[test]
fn help_overlay_hidden_in_normal_mode() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 80, 30);
    assert!(!buffer_contains(&buf, "Navigation"));
}

// ---------------------------------------------------------------------------
// Finish task tests
// ---------------------------------------------------------------------------

#[test]
fn finish_complete_moves_to_done() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], TEST_TIMEOUT);

    let cmds = app.update(Message::FinishComplete(TaskId(1)));
    let task = app.tasks().iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    // Worktree is preserved — will be cleaned up during archive
    assert!(task.worktree.is_some());
    assert!(task.tmux_window.is_none());
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn finish_failed_with_conflict_sets_flag() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], TEST_TIMEOUT);

    app.update(Message::FinishFailed {
        id: TaskId(1),
        error: "Rebase conflict".to_string(),
        is_conflict: true,
    });
    assert!(app.find_task(TaskId(1)).is_some_and(|t| t.sub_status == SubStatus::Conflict));
    assert!(app.status_message.as_ref().unwrap().contains("Rebase conflict"));
}

#[test]
fn finish_failed_without_conflict_does_not_set_flag() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], TEST_TIMEOUT);

    app.update(Message::FinishFailed {
        id: TaskId(1),
        error: "Not on main".to_string(),
        is_conflict: false,
    });
    assert!(!app.find_task(TaskId(1)).is_some_and(|t| t.sub_status == SubStatus::Conflict));
}

#[test]
fn conflict_flag_clears_on_dispatch() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], TEST_TIMEOUT);

    app.update(Message::FinishFailed {
        id: TaskId(1),
        error: "conflict".to_string(),
        is_conflict: true,
    });
    assert!(app.find_task(TaskId(1)).is_some_and(|t| t.sub_status == SubStatus::Conflict));

    app.update(Message::Resumed { id: TaskId(1), tmux_window: "task-1".to_string() });
    assert!(!app.find_task(TaskId(1)).is_some_and(|t| t.sub_status == SubStatus::Conflict));
}

#[test]
fn conflict_flag_clears_on_move_backward() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], TEST_TIMEOUT);

    app.update(Message::FinishFailed {
        id: TaskId(1),
        error: "conflict".to_string(),
        is_conflict: true,
    });

    app.update(Message::MoveTask { id: TaskId(1), direction: MoveDirection::Backward });
    assert!(!app.find_task(TaskId(1)).is_some_and(|t| t.sub_status == SubStatus::Conflict));
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
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    // Task is in Running column (column 1), navigate there
    app.selection_mut().set_column(1);
    app.update(Message::ConfirmDeleteStart);
    assert_eq!(app.input.mode, InputMode::ConfirmDelete);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Delete \"Task 4\" [running] (has worktree)? (y/n)")
    );
}

#[test]
fn focused_column_has_tinted_background() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
    ], TEST_TIMEOUT);
    // Use wider terminal so 8 columns have enough room for content.
    // Columns use Ratio constraints (3/18, 2/18, ...) so they aren't equal width.
    let buf = render_to_buffer(&mut app, 240, 30);

    // Focused column (Backlog, col 0) should have a tinted bg.
    // Check a row well below the cursor card to avoid cursor highlight.
    let expected_bg = Color::Rgb(28, 30, 44);
    let cell = &buf[(1, 15)];
    // Backlog is 3/18 of 240 = 40px. Check well past that at x=120 (middle of board).
    let cell2 = &buf[(120, 15)];

    assert_eq!(cell.bg, expected_bg, "Focused column should have tinted background");
    assert_ne!(cell2.bg, expected_bg, "Unfocused column should NOT have tinted background");
}

// ---------------------------------------------------------------------------
// Done confirmation tests
// ---------------------------------------------------------------------------

#[test]
fn move_review_to_done_enters_confirm_mode() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
    ], TEST_TIMEOUT);
    app.selection_mut().set_column(2); // Review column

    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert!(cmds.is_empty());
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(1))));
    assert!(app.status_message.as_deref().unwrap().contains("Done"));
}

#[test]
fn confirm_done_y_moves_task() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
    ], TEST_TIMEOUT);
    app.selection_mut().set_column(2);

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
    ], TEST_TIMEOUT);
    app.selection_mut().set_column(2);

    app.input.mode = InputMode::ConfirmDone(TaskId(1));
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert!(cmds.is_empty());
}

#[test]
fn move_backlog_to_running_no_confirmation() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.selection_mut().set_column(0); // Backlog column

    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn confirm_done_kills_tmux_but_preserves_worktree() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-test".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], TEST_TIMEOUT);
    app.selection_mut().set_column(2);

    // Enter confirm mode and confirm
    app.update(Message::MoveTask { id: TaskId(1), direction: MoveDirection::Forward });
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(1))));

    let cmds = app.update(Message::ConfirmDone);
    // No Cleanup command — worktree stays for archive to clean up later
    assert!(!cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    // Tmux window should be killed
    assert!(cmds.iter().any(|c| matches!(c, Command::KillTmuxWindow { .. })));
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    // Worktree is preserved (not taken), tmux_window cleared
    assert!(task.worktree.is_some());
    assert!(task.tmux_window.is_none());
}

#[test]
fn batch_move_with_review_tasks_enters_confirm_done() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
        make_task(2, TaskStatus::Review),
    ], TEST_TIMEOUT);
    app.selection_mut().set_column(2);
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("2 tasks"));
    assert!(app.status_message.as_deref().unwrap().contains("Done"));
}

#[test]
fn batch_confirm_done_moves_all_review_tasks() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
        make_task(2, TaskStatus::Review),
    ], TEST_TIMEOUT);
    app.selection_mut().set_column(2);
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    // Trigger batch move
    app.update(Message::BatchMoveTasks {
        ids: vec![TaskId(1), TaskId(2)],
        direction: MoveDirection::Forward,
    });
    // Confirm
    let cmds = app.update(Message::ConfirmDone);
    assert_eq!(app.input.mode, InputMode::Normal);
    for id in [TaskId(1), TaskId(2)] {
        let task = app.tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(task.status, TaskStatus::Done);
    }
    assert!(cmds.len() >= 2); // two PersistTask commands
}

#[test]
fn batch_move_mixed_statuses_moves_non_review_immediately() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Running),
        make_task(2, TaskStatus::Review),
    ], TEST_TIMEOUT);
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    let cmds = app.update(Message::BatchMoveTasks {
        ids: vec![TaskId(1), TaskId(2)],
        direction: MoveDirection::Forward,
    });
    // Running→Review moved immediately
    let t1 = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(t1.status, TaskStatus::Review);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.id == TaskId(1))));

    // Review→Done waiting for confirmation
    let t2 = app.tasks.iter().find(|t| t.id == TaskId(2)).unwrap();
    assert_eq!(t2.status, TaskStatus::Review); // not moved yet
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(_)));
}

// --- Status message auto-clear ---

#[test]
fn status_message_clears_after_timeout_on_tick() {
    let mut app = make_app();
    // Simulate a status message that was set 6 seconds ago
    app.status_message = Some("Task 1 finished".to_string());
    app.status_message_set_at = Some(Instant::now() - Duration::from_secs(6));

    // Tick should clear it since it's past the 5-second timeout
    app.update(Message::Tick);
    assert!(app.status_message.is_none(), "status_message should auto-clear after timeout");
}

#[test]
fn status_message_persists_before_timeout() {
    let mut app = make_app();
    // Set a message just now
    app.status_message = Some("Task 1 finished".to_string());
    app.status_message_set_at = Some(Instant::now());

    // Tick should NOT clear it since timeout hasn't elapsed
    app.update(Message::Tick);
    assert_eq!(app.status_message.as_deref(), Some("Task 1 finished"));
}

#[test]
fn status_message_does_not_clear_during_interactive_mode() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDelete;
    app.status_message = Some("Delete task? (y/n)".to_string());
    app.status_message_set_at = Some(Instant::now() - Duration::from_secs(10));

    // Tick should NOT clear it during an interactive mode
    app.update(Message::Tick);
    assert!(app.status_message.is_some(), "should not clear during interactive mode");
}

// ---------------------------------------------------------------------------
// Select-all toggle
// ---------------------------------------------------------------------------

#[test]
fn on_select_all_defaults_to_false() {
    let app = make_app();
    assert!(!app.on_select_all());
}

#[test]
fn select_all_column_selects_all_tasks_in_column() {
    let mut app = make_app();
    // Cursor is on Backlog (column 0) which has tasks 1, 2
    app.update(Message::SelectAllColumn);
    assert!(app.selected_tasks.contains(&TaskId(1)));
    assert!(app.selected_tasks.contains(&TaskId(2)));
    assert_eq!(app.selected_tasks.len(), 2);
}

#[test]
fn select_all_column_deselects_when_all_selected() {
    let mut app = make_app();
    app.update(Message::SelectAllColumn);
    assert_eq!(app.selected_tasks.len(), 2);

    app.update(Message::SelectAllColumn);
    assert!(app.selected_tasks.is_empty());
}

#[test]
fn select_all_column_selects_remaining_when_partially_selected() {
    let mut app = make_app();
    app.update(Message::ToggleSelect(TaskId(1)));
    assert_eq!(app.selected_tasks.len(), 1);

    app.update(Message::SelectAllColumn);
    assert!(app.selected_tasks.contains(&TaskId(1)));
    assert!(app.selected_tasks.contains(&TaskId(2)));
    assert_eq!(app.selected_tasks.len(), 2);
}

#[test]
fn select_all_column_noop_on_empty_column() {
    let mut app = make_app();
    // Navigate to Review column (empty in make_app)
    app.update(Message::NavigateColumn(2));
    app.update(Message::SelectAllColumn);
    assert!(app.selected_tasks.is_empty());
}

#[test]
fn select_all_column_only_affects_current_column() {
    let mut app = make_app();
    // TaskId(3) is in Running column, pre-select it
    app.update(Message::ToggleSelect(TaskId(3)));
    // SelectAllColumn selects all in current (Backlog) column
    app.update(Message::SelectAllColumn);
    assert!(app.selected_tasks.contains(&TaskId(1)));
    assert!(app.selected_tasks.contains(&TaskId(2)));
    assert!(app.selected_tasks.contains(&TaskId(3)));
    assert_eq!(app.selected_tasks.len(), 3);
}

#[test]
fn select_all_deselect_only_affects_current_column() {
    let mut app = make_app();
    app.update(Message::ToggleSelect(TaskId(3)));
    app.update(Message::SelectAllColumn);
    assert_eq!(app.selected_tasks.len(), 3);

    app.update(Message::SelectAllColumn);
    assert_eq!(app.selected_tasks.len(), 1);
    assert!(app.selected_tasks.contains(&TaskId(3)));
}

#[test]
fn key_a_selects_all_in_column() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('a')));
    assert!(app.selected_tasks.contains(&TaskId(1)));
    assert!(app.selected_tasks.contains(&TaskId(2)));
}

#[test]
fn key_a_toggles_off_when_all_selected() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('a')));
    assert_eq!(app.selected_tasks.len(), 2);
    app.handle_key(make_key(KeyCode::Char('a')));
    assert!(app.selected_tasks.is_empty());
}

#[test]
fn navigate_up_from_row_zero_enters_select_all_toggle() {
    let mut app = make_app();
    assert!(!app.on_select_all());
    app.handle_key(make_key(KeyCode::Char('k')));
    assert!(app.on_select_all());
}

#[test]
fn navigate_down_from_toggle_exits_to_row_zero() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('k')));
    assert!(app.on_select_all());
    app.handle_key(make_key(KeyCode::Char('j')));
    assert!(!app.on_select_all());
    assert_eq!(app.selected_row()[0], 0);
}

#[test]
fn column_switch_preserves_on_select_all() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('k')));
    assert!(app.on_select_all());
    app.handle_key(make_key(KeyCode::Char('l')));
    assert!(app.on_select_all());
}

#[test]
fn enter_on_toggle_triggers_select_all() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('k')));
    app.handle_key(make_key(KeyCode::Enter));
    assert!(app.selected_tasks.contains(&TaskId(1)));
    assert!(app.selected_tasks.contains(&TaskId(2)));
}

#[test]
fn esc_clears_selection_and_exits_toggle() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('k')));
    assert!(app.on_select_all());
    app.handle_key(make_key(KeyCode::Esc));
    assert!(app.selected_tasks.is_empty());
    assert!(!app.on_select_all());
}

#[test]
fn space_is_noop_when_on_select_all() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('k')));
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.selected_tasks.is_empty());
}

#[test]
fn dispatch_is_noop_when_on_select_all() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('k')));
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
}

#[test]
fn render_shows_select_all_toggle_in_focused_column() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "[ ]"));
    assert!(!buffer_contains(&buf, "Select [a]ll"));
}

#[test]
fn render_shows_checked_toggle_when_all_selected() {
    let mut app = make_app();
    app.update(Message::SelectAllColumn);
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "[x]"));
}

#[test]
fn render_shows_unchecked_toggle_when_not_all_selected() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "[ ]"));
}

#[test]
fn action_hints_include_select_all() {
    let app = make_app();
    let task = app.selected_task();
    let spans = ui::action_hints(task, Color::Blue);
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("select all"), "action hints should include 'select all'");
}

// ---------------------------------------------------------------------------
// Column scrolling tests
// ---------------------------------------------------------------------------

#[test]
fn column_scrolls_to_keep_cursor_visible() {
    // Create 20 backlog tasks — more than fit in a 20-row terminal
    let tasks: Vec<Task> = (1..=20)
        .map(|id| make_task(id, TaskStatus::Backlog))
        .collect();
    let mut app = App::new(tasks, TEST_TIMEOUT);

    // Navigate down to the last task (row 19, past visible area)
    for _ in 0..19 {
        app.update(Message::NavigateRow(1));
    }

    // Render in a small terminal (height 20 with header/detail/status ~10 lines
    // leaves roughly 10 lines for the column, fitting ~4-5 two-line task cards)
    let buf = render_to_buffer(&mut app, 120, 20);

    // The cursor should be on "Task 20" and it should be visible in the buffer
    assert!(
        buffer_contains(&buf, "Task 20"),
        "cursor task should be visible after scrolling down"
    );
}

#[test]
fn column_scrolls_back_up_when_cursor_moves_up() {
    let tasks: Vec<Task> = (1..=20)
        .map(|id| make_task(id, TaskStatus::Backlog))
        .collect();
    let mut app = App::new(tasks, TEST_TIMEOUT);

    // Navigate to the bottom
    for _ in 0..19 {
        app.update(Message::NavigateRow(1));
    }
    // Render once to establish scroll state
    let _ = render_to_buffer(&mut app, 120, 20);

    // Navigate back to the top
    for _ in 0..19 {
        app.update(Message::NavigateRow(-1));
    }
    let buf = render_to_buffer(&mut app, 120, 20);

    assert!(
        buffer_contains(&buf, "Task 1"),
        "first task should be visible after scrolling back up"
    );
}

#[test]
fn toggle_notifications_flips_state() {
    let mut app = make_app();
    assert!(app.notifications_enabled()); // default: true
    app.update(Message::ToggleNotifications);
    assert!(!app.notifications_enabled());
    app.update(Message::ToggleNotifications);
    assert!(app.notifications_enabled());
}

#[test]
fn refresh_tasks_emits_notification_on_review_transition() {
    let mut app = make_app();
    // Task 3 starts as Running
    assert_eq!(app.tasks()[2].status, TaskStatus::Running);

    // Simulate DB refresh where task 3 moved to Review
    let mut updated = app.tasks().to_vec();
    updated[2].status = TaskStatus::Review;
    let cmds = app.update(Message::RefreshTasks(updated));

    let notif_cmds: Vec<_> = cmds.iter().filter(|c| matches!(c, Command::SendNotification { .. })).collect();
    assert_eq!(notif_cmds.len(), 1);
    match &notif_cmds[0] {
        Command::SendNotification { title, urgent, .. } => {
            assert!(title.contains("Task 3"));
            assert!(!urgent);
        }
        _ => unreachable!(),
    }
}

#[test]
fn refresh_tasks_emits_urgent_notification_on_needs_input() {
    let mut app = make_app();

    let mut updated = app.tasks().to_vec();
    updated[2].sub_status = SubStatus::NeedsInput;
    let cmds = app.update(Message::RefreshTasks(updated));

    let notif_cmds: Vec<_> = cmds.iter().filter(|c| matches!(c, Command::SendNotification { .. })).collect();
    assert_eq!(notif_cmds.len(), 1);
    match &notif_cmds[0] {
        Command::SendNotification { urgent, .. } => {
            assert!(urgent);
        }
        _ => unreachable!(),
    }
}

#[test]
fn refresh_tasks_does_not_duplicate_notifications() {
    let mut app = make_app();

    let mut updated = app.tasks().to_vec();
    updated[2].status = TaskStatus::Review;
    app.update(Message::RefreshTasks(updated.clone()));
    // Second refresh with same state should not re-notify
    let cmds = app.update(Message::RefreshTasks(updated));
    let notif_cmds: Vec<_> = cmds.iter().filter(|c| matches!(c, Command::SendNotification { .. })).collect();
    assert_eq!(notif_cmds.len(), 0);
}

#[test]
fn refresh_tasks_does_not_duplicate_needs_input_notifications() {
    let mut app = make_app();

    let mut updated = app.tasks().to_vec();
    updated[2].sub_status = SubStatus::NeedsInput;
    app.update(Message::RefreshTasks(updated.clone()));
    // Second refresh with same state should not re-notify
    let cmds = app.update(Message::RefreshTasks(updated));
    let notif_cmds: Vec<_> = cmds
        .iter()
        .filter(|c| matches!(c, Command::SendNotification { .. }))
        .collect();
    assert_eq!(notif_cmds.len(), 0);
}

#[test]
fn refresh_tasks_renotifies_needs_input_after_clearing() {
    let mut app = make_app();

    // First transition to NeedsInput
    let mut updated = app.tasks().to_vec();
    updated[2].sub_status = SubStatus::NeedsInput;
    let cmds = app.update(Message::RefreshTasks(updated.clone()));
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(c, Command::SendNotification { .. }))
            .count(),
        1
    );

    // Clear NeedsInput (agent resumes)
    updated[2].sub_status = SubStatus::Active;
    app.update(Message::RefreshTasks(updated.clone()));

    // Second transition to NeedsInput should re-notify
    updated[2].sub_status = SubStatus::NeedsInput;
    let cmds = app.update(Message::RefreshTasks(updated));
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(c, Command::SendNotification { .. }))
            .count(),
        1
    );
}

#[test]
fn refresh_tasks_skips_notification_when_disabled() {
    let mut app = make_app();
    app.update(Message::ToggleNotifications); // disable

    let mut updated = app.tasks().to_vec();
    updated[2].status = TaskStatus::Review;
    let cmds = app.update(Message::RefreshTasks(updated));

    let notif_cmds: Vec<_> = cmds.iter().filter(|c| matches!(c, Command::SendNotification { .. })).collect();
    assert_eq!(notif_cmds.len(), 0);
}

#[test]
fn key_n_uppercase_toggles_notifications() {
    let mut app = make_app();
    assert!(app.notifications_enabled());
    let cmds = app.handle_key(make_key(KeyCode::Char('N')));
    assert!(!app.notifications_enabled());
    // Should emit PersistSetting command
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistSetting { .. })));
    // Should show status message
    assert!(app.status_message().unwrap().contains("disabled"));
}

#[test]
fn refresh_tasks_clears_notified_when_task_leaves_review() {
    let mut app = make_app();

    // Move to review — triggers notification
    let mut updated = app.tasks().to_vec();
    updated[2].status = TaskStatus::Review;
    app.update(Message::RefreshTasks(updated));

    // Move to done — should clear notified state
    let mut updated2 = app.tasks().to_vec();
    updated2[2].status = TaskStatus::Done;
    app.update(Message::RefreshTasks(updated2));

    // Move back to review — should re-notify
    let mut updated3 = app.tasks().to_vec();
    updated3[2].status = TaskStatus::Review;
    let cmds = app.update(Message::RefreshTasks(updated3));
    let notif_cmds: Vec<_> = cmds.iter().filter(|c| matches!(c, Command::SendNotification { .. })).collect();
    assert_eq!(notif_cmds.len(), 1);
}

#[test]
fn refresh_tasks_clears_notified_state_even_when_disabled() {
    let mut app = make_app();

    // Task transitions to review while notifications enabled — gets notified
    let mut updated = app.tasks().to_vec();
    updated[2].status = TaskStatus::Review;
    let cmds = app.update(Message::RefreshTasks(updated));
    assert_eq!(cmds.iter().filter(|c| matches!(c, Command::SendNotification { .. })).count(), 1);

    // Disable notifications
    app.update(Message::ToggleNotifications);

    // Task leaves review while disabled
    let mut updated2 = app.tasks().to_vec();
    updated2[2].status = TaskStatus::Done;
    app.update(Message::RefreshTasks(updated2));

    // Re-enable notifications
    app.update(Message::ToggleNotifications);

    // Task returns to review — should re-notify because notified state was cleared
    let mut updated3 = app.tasks().to_vec();
    updated3[2].status = TaskStatus::Review;
    let cmds = app.update(Message::RefreshTasks(updated3));
    let notif_cmds: Vec<_> = cmds.iter().filter(|c| matches!(c, Command::SendNotification { .. })).collect();
    assert_eq!(notif_cmds.len(), 1, "Should re-notify after notified state was cleared while disabled");
}

#[test]
fn summary_row_shows_bell_when_notifications_enabled() {
    let mut app = make_app(); // notifications_enabled defaults to true
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "\u{1F514}")); // 🔔
}

#[test]
fn summary_row_shows_muted_bell_and_hint_when_disabled() {
    let mut app = make_app();
    app.update(Message::ToggleNotifications); // disable
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "\u{1F515}")); // 🔕
    assert!(buffer_contains(&buf, "[N]"));
}

// -----------------------------------------------------------------------
// PR handler tests
// -----------------------------------------------------------------------

#[test]
fn pr_created_stores_url() {
    let task = make_task(1, TaskStatus::Review);
    let mut app = App::new(vec![task], TEST_TIMEOUT);

    let cmds = app.update(Message::PrCreated {
        id: TaskId(1),
        pr_url: "https://github.com/org/repo/pull/42".to_string(),
    });

    let task = app.find_task(TaskId(1)).unwrap();
    assert_eq!(task.pr_url.as_deref(), Some("https://github.com/org/repo/pull/42"));
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn pr_failed_shows_error() {
    let task = make_task(1, TaskStatus::Review);
    let mut app = App::new(vec![task], TEST_TIMEOUT);

    app.update(Message::PrFailed {
        id: TaskId(1),
        error: "Push failed".to_string(),
    });

    assert!(app.status_message().unwrap().contains("Push failed"));
}

#[test]
fn pr_merged_moves_to_done_and_detaches() {
    let mut task = make_task(1, TaskStatus::Review);
    task.tmux_window = Some("task-1".to_string());
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);

    let cmds = app.update(Message::PrMerged(TaskId(1)));

    let task = app.find_task(TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(task.tmux_window.is_none(), "tmux window should be cleared");
    assert!(task.worktree.is_some(), "worktree should be preserved");
    assert!(task.pr_url.is_some(), "pr_url should be preserved");
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
    assert!(cmds.iter().any(|c| matches!(c, Command::SendNotification { .. })));
}

#[test]
fn pr_merged_preserves_worktree() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);

    let cmds = app.update(Message::PrMerged(TaskId(1)));

    // Should NOT emit a Cleanup command
    assert!(!cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
}

#[test]
fn card_shows_pr_badge() {
    let mut task = make_task(1, TaskStatus::Review);
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    // Navigate to Review column (index 2)
    for _ in 0..2 {
        app.update(Message::NavigateColumn(1));
    }

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "PR #42"), "Card should show PR #42 badge");
}

#[test]
fn card_shows_merged_pr_badge() {
    let mut task = make_task(1, TaskStatus::Done);
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    // Navigate to Done column (visual index 7)
    for _ in 0..7 {
        app.update(Message::NavigateColumn(1));
    }

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "PR #42 merged"), "Done card should show merged PR badge");
}

#[test]
fn status_bar_shows_wrap_up_hint_for_review_task() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    // Navigate to Review column (index 2)
    for _ in 0..2 {
        app.update(Message::NavigateColumn(1));
    }

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "wrap up"), "Status bar should show wrap up hint for Review tasks");
}

#[test]
fn detail_panel_shows_pr_url() {
    let mut task = make_task(1, TaskStatus::Review);
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    // Navigate to Review column (index 2) and open detail panel
    for _ in 0..2 {
        app.update(Message::NavigateColumn(1));
    }
    app.update(Message::ToggleDetail);

    let buf = render_to_buffer(&mut app, 200, 20);
    assert!(buffer_contains(&buf, "PR:"), "Detail panel should show PR label");
    assert!(buffer_contains(&buf, "pull/42"), "Detail panel should show PR URL");
}

#[test]
fn pr_polling_skips_done_tasks() {
    let mut task = make_task(1, TaskStatus::Done);
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);

    let cmds = app.update(Message::Tick);
    // Should NOT contain any CheckPrStatus command
    assert!(!cmds.iter().any(|c| matches!(c, Command::CheckPrStatus { .. })));
}

#[test]
fn pr_polling_emits_check_for_review_tasks() {
    let mut task = make_task(1, TaskStatus::Review);
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    let mut app = App::new(vec![task], TEST_TIMEOUT);

    let cmds = app.update(Message::Tick);
    assert!(cmds.iter().any(|c| matches!(c, Command::CheckPrStatus { ref pr_url, .. } if pr_url == "https://github.com/org/repo/pull/42")));
}

// --- repo_filter ---

#[test]
fn repo_filter_empty_shows_all_tasks() {
    let app = make_app();
    // repo_filter is empty by default => all tasks visible
    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 4); // tasks 1,2,3,4 (Done tasks are visible, only Archived are excluded)
}

#[test]
fn repo_filter_hides_non_matching_tasks() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.repo_path = "/repo-a".to_string();
    let mut t2 = make_task(2, TaskStatus::Backlog);
    t2.repo_path = "/repo-b".to_string();
    app.tasks = vec![t1, t2];
    app.repo_filter.insert("/repo-a".to_string());

    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, TaskId(1));
}

#[test]
fn repo_filter_applies_to_epics_in_column_items() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let now = chrono::Utc::now();
    app.epics = vec![
        Epic {
            id: EpicId(1), title: "A".into(), description: "".into(),
            repo_path: "/repo-a".into(), status: TaskStatus::Backlog, plan: None, sort_order: None, created_at: now, updated_at: now,
        },
        Epic {
            id: EpicId(2), title: "B".into(), description: "".into(),
            repo_path: "/repo-b".into(), status: TaskStatus::Backlog, plan: None, sort_order: None, created_at: now, updated_at: now,
        },
    ];
    app.repo_filter.insert("/repo-a".to_string());

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert_eq!(items.len(), 1); // only epic A
}

#[test]
fn repo_filter_applies_to_archived_tasks() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let mut t1 = make_task(1, TaskStatus::Archived);
    t1.repo_path = "/repo-a".to_string();
    let mut t2 = make_task(2, TaskStatus::Archived);
    t2.repo_path = "/repo-b".to_string();
    app.tasks = vec![t1, t2];
    app.repo_filter.insert("/repo-a".to_string());

    let archived = app.archived_tasks();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, TaskId(1));
}

// --- repo filter keybindings ---

#[test]
fn f_key_opens_repo_filter() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo".to_string()];
    app.handle_key(make_key(KeyCode::Char('f')));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn repo_filter_number_key_toggles_repo() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('1')));
    assert!(app.repo_filter.contains("/repo-a"));

    app.handle_key(make_key(KeyCode::Char('1')));
    assert!(!app.repo_filter.contains("/repo-a"));
}

#[test]
fn repo_filter_a_key_toggles_all() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('a')));
    assert_eq!(app.repo_filter.len(), 2);
}

#[test]
fn repo_filter_enter_closes() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistStringSetting { .. })));
}

#[test]
fn repo_filter_esc_closes() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn repo_filter_out_of_range_number_ignored() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('5')));
    assert!(app.repo_filter.is_empty());
}

#[test]
fn summary_row_shows_filter_indicator() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.repo_filter.insert("/a".to_string());
    app.repo_filter.insert("/b".to_string());

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "2/3 repos"), "Expected filter indicator in summary");
}

// --- wrap up ---

#[test]
fn w_key_on_review_task_with_worktree_enters_wrap_up() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], TEST_TIMEOUT);
    // Navigate to Review column (index 2)
    app.update(Message::NavigateColumn(2));

    app.handle_key(make_key(KeyCode::Char('W')));
    assert!(matches!(app.input.mode, InputMode::ConfirmWrapUp(TaskId(1))));
}

#[test]
fn w_key_on_non_review_task_is_noop() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);

    app.handle_key(make_key(KeyCode::Char('W')));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn wrap_up_r_emits_finish_command() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], TEST_TIMEOUT);
    app.update(Message::NavigateColumn(4));

    app.update(Message::StartWrapUp(TaskId(1)));
    let cmds = app.update(Message::WrapUpRebase);
    assert!(cmds.iter().any(|c| matches!(c, Command::Finish { .. })));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn wrap_up_p_emits_create_pr_command() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], TEST_TIMEOUT);
    app.update(Message::NavigateColumn(4));

    app.update(Message::StartWrapUp(TaskId(1)));
    let cmds = app.update(Message::WrapUpPr);
    assert!(cmds.iter().any(|c| matches!(c, Command::CreatePr { .. })));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn wrap_up_esc_cancels() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], TEST_TIMEOUT);
    app.update(Message::NavigateColumn(4));

    app.update(Message::StartWrapUp(TaskId(1)));
    app.update(Message::CancelWrapUp);
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn wrap_up_rebase_clears_conflict_flag() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], TEST_TIMEOUT);

    app.find_task_mut(TaskId(1)).unwrap().sub_status = SubStatus::Conflict;
    app.update(Message::StartWrapUp(TaskId(1)));
    app.update(Message::WrapUpRebase);
    assert!(!app.find_task(TaskId(1)).is_some_and(|t| t.sub_status == SubStatus::Conflict));
}

#[test]
fn wrap_up_available_on_running_blocked() {
    let mut app = make_app();
    let id = TaskId(3); // Running
    app.find_task_mut(id).unwrap().sub_status = SubStatus::NeedsInput;
    app.find_task_mut(id).unwrap().worktree = Some("/tmp/wt".to_string());
    app.selection_mut().set_column(2); // Blocked column
    app.update(Message::StartWrapUp(id));
    assert!(matches!(app.mode(), InputMode::ConfirmWrapUp(_)));
}

#[test]
fn wrap_up_available_on_running_active() {
    let mut app = make_app();
    let id = TaskId(3); // Running, Active by default
    app.find_task_mut(id).unwrap().worktree = Some("/tmp/wt".to_string());
    app.update(Message::StartWrapUp(id));
    assert!(matches!(app.mode(), InputMode::ConfirmWrapUp(_)));
}

// --- sort_order ---

#[test]
fn column_items_sorted_by_sort_order() {
    let mut app = make_app();
    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.title = "First".to_string();
    t1.sort_order = Some(200);
    let mut t2 = make_task(2, TaskStatus::Backlog);
    t2.title = "Second".to_string();
    t2.sort_order = Some(100);
    app.tasks = vec![t1, t2];

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert_eq!(items.len(), 2);
    match &items[0] {
        ColumnItem::Task(t) => assert_eq!(t.title, "Second"),
        _ => panic!("expected task"),
    }
    match &items[1] {
        ColumnItem::Task(t) => assert_eq!(t.title, "First"),
        _ => panic!("expected task"),
    }
}

#[test]
fn column_items_null_sort_order_uses_id() {
    let mut app = make_app();
    let mut t1 = make_task(10, TaskStatus::Backlog);
    t1.title = "High ID".to_string();
    t1.sort_order = None;
    let mut t2 = make_task(5, TaskStatus::Backlog);
    t2.title = "Low ID".to_string();
    t2.sort_order = None;
    app.tasks = vec![t1, t2];

    let items = app.column_items_for_status(TaskStatus::Backlog);
    match &items[0] {
        ColumnItem::Task(t) => assert_eq!(t.title, "Low ID"),
        _ => panic!("expected task"),
    }
}

// ---------------------------------------------------------------------------
// Reorder item (J/K) tests
// ---------------------------------------------------------------------------

#[test]
fn reorder_task_down_swaps_sort_order() {
    let mut app = make_app();
    let t1 = make_task(1, TaskStatus::Backlog);
    let t2 = make_task(2, TaskStatus::Backlog);
    app.tasks = vec![t1, t2];

    // Cursor on first task (row 0, column 0 = Backlog)
    let cmds = app.update(Message::ReorderItem(1));

    // After reorder, task 1 should have a higher sort value than task 2
    let t1 = app.find_task(TaskId(1)).unwrap();
    let t2 = app.find_task(TaskId(2)).unwrap();
    let eff1 = t1.sort_order.unwrap_or(t1.id.0);
    let eff2 = t2.sort_order.unwrap_or(t2.id.0);
    assert!(eff1 > eff2, "task 1 ({eff1}) should be after task 2 ({eff2}) after move down");
    // Should emit PersistTask for both
    assert_eq!(cmds.iter().filter(|c| matches!(c, Command::PersistTask(_))).count(), 2);
    // Cursor should have moved down
    assert_eq!(app.selection().row(0), 1);
}

#[test]
fn reorder_task_up_at_top_is_noop() {
    let mut app = make_app();
    let t1 = make_task(1, TaskStatus::Backlog);
    app.tasks = vec![t1];

    let cmds = app.update(Message::ReorderItem(-1));
    assert!(cmds.is_empty());
}

#[test]
fn reorder_task_down_at_bottom_is_noop() {
    let mut app = make_app();
    let t1 = make_task(1, TaskStatus::Backlog);
    app.tasks = vec![t1];

    let cmds = app.update(Message::ReorderItem(1));
    assert!(cmds.is_empty());
}

#[test]
fn reorder_task_up_swaps_sort_order() {
    let mut app = make_app();
    let t1 = make_task(1, TaskStatus::Backlog);
    let t2 = make_task(2, TaskStatus::Backlog);
    app.tasks = vec![t1, t2];

    // Move cursor to row 1 (second task), then reorder up
    app.selection_mut().set_row(0, 1);
    let cmds = app.update(Message::ReorderItem(-1));

    // After reorder, task 2 should have a lower sort value than task 1
    let t1 = app.find_task(TaskId(1)).unwrap();
    let t2 = app.find_task(TaskId(2)).unwrap();
    let eff1 = t1.sort_order.unwrap_or(t1.id.0);
    let eff2 = t2.sort_order.unwrap_or(t2.id.0);
    assert!(eff2 < eff1, "task 2 ({eff2}) should be before task 1 ({eff1}) after move up");
    assert_eq!(cmds.iter().filter(|c| matches!(c, Command::PersistTask(_))).count(), 2);
    // Cursor should have moved up
    assert_eq!(app.selection().row(0), 0);
}

// --- Epic dispatch: next backlog subtask ---

#[test]
fn dispatch_epic_with_backlog_subtasks_dispatches_first_by_sort_order() {
    let mut app = make_app();

    // Create epic with a plan so subtask dispatch path is taken
    let mut epic = make_epic(1);
    epic.plan = Some("docs/plans/epic-1.md".to_string());
    app.epics = vec![epic];

    // Create two backlog subtasks with different sort orders (both have plans)
    let mut t1 = make_task(10, TaskStatus::Backlog);
    t1.epic_id = Some(EpicId(1));
    t1.sort_order = Some(200);
    t1.title = "Second task".to_string();
    t1.plan = Some("docs/plans/task-10.md".to_string());
    let mut t2 = make_task(11, TaskStatus::Backlog);
    t2.epic_id = Some(EpicId(1));
    t2.sort_order = Some(100);
    t2.title = "First task".to_string();
    t2.plan = Some("docs/plans/task-11.md".to_string());
    app.tasks = vec![t1, t2];

    let cmds = app.update(Message::DispatchEpic(EpicId(1)));

    // Should dispatch the task with lower sort_order (task 11, sort_order=100)
    assert!(cmds.iter().any(|c| matches!(c, Command::Dispatch { task } if task.id == TaskId(11))));
}

#[test]
fn dispatch_epic_no_subtasks_falls_back_to_planning() {
    let mut app = make_app();

    let epic = make_epic(1);
    app.epics = vec![epic];
    // No subtasks

    let cmds = app.update(Message::DispatchEpic(EpicId(1)));

    // Should fall back to planning dispatch
    assert!(cmds.iter().any(|c| matches!(c, Command::DispatchEpic { .. })));
}

#[test]
fn dispatch_epic_all_done_shows_message() {
    let mut app = make_app();

    let mut epic = make_epic(1);
    epic.status = TaskStatus::Done;
    app.epics = vec![epic];

    let mut t1 = make_task(10, TaskStatus::Done);
    t1.epic_id = Some(EpicId(1));
    app.tasks = vec![t1];

    let cmds = app.update(Message::DispatchEpic(EpicId(1)));

    // Epic status is Done — should not dispatch
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("No backlog tasks"));
}

// ---------------------------------------------------------------------------
// Review board tests
// ---------------------------------------------------------------------------

use crate::models::ReviewDecision;

fn make_review_pr(number: i64, author: &str, decision: ReviewDecision) -> crate::models::ReviewPr {
    crate::models::ReviewPr {
        number,
        title: format!("PR {number}"),
        author: author.to_string(),
        repo: "acme/app".to_string(),
        url: format!("https://github.com/acme/app/pull/{number}"),
        is_draft: false,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        additions: 10,
        deletions: 5,
        review_decision: decision,
        labels: vec![],
        body: String::new(),
        head_ref: String::new(),
        ci_status: crate::models::CiStatus::None,
        reviewers: vec![],
    }
}

#[test]
fn switch_to_review_board_preserves_task_selection() {
    let mut app = make_app();
    // Move cursor to column 1
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), 1);

    // Switch to review board
    app.update(Message::SwitchToReviewBoard);
    assert!(matches!(app.view_mode(), ViewMode::ReviewBoard { .. }));

    // Switch back
    app.update(Message::SwitchToTaskBoard);
    assert!(matches!(app.view_mode(), ViewMode::Board(_)));
    // Task board cursor should be restored
    assert_eq!(app.selected_column(), 1);
}

#[test]
fn review_prs_loaded_updates_state() {
    let mut app = make_app();
    let prs = vec![make_review_pr(42, "alice", ReviewDecision::ReviewRequired)];
    app.update(Message::ReviewPrsLoaded(prs));
    assert_eq!(app.review_prs().len(), 1);
    assert_eq!(app.review_prs()[0].number, 42);
    assert!(!app.review_board_loading());
}

#[test]
fn review_prs_fetch_failed_sets_error() {
    let mut app = make_app();
    app.update(Message::ReviewPrsFetchFailed("auth error".to_string()));
    assert!(!app.review_board_loading());
    assert!(app.status_message().unwrap().contains("auth error"));
    assert_eq!(app.last_review_error(), Some("auth error"));
}

#[test]
fn switch_to_review_board_sets_loading() {
    let mut app = make_app();
    let cmds = app.update(Message::SwitchToReviewBoard);
    assert!(app.review_board_loading());
    assert!(cmds.iter().any(|c| matches!(c, Command::FetchReviewPrs)));
}

#[test]
fn tab_switches_to_review_board() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Tab));
    assert!(matches!(app.view_mode(), ViewMode::ReviewBoard { .. }));
    assert!(cmds.iter().any(|c| matches!(c, Command::FetchReviewPrs)));
}

#[test]
fn tab_in_review_board_switches_back() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Tab)); // to review board
    app.handle_key(make_key(KeyCode::Tab)); // back to task board
    assert!(matches!(app.view_mode(), ViewMode::Board(_)));
}

#[test]
fn esc_in_review_board_switches_back() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Tab)); // to review board
    app.handle_key(make_key(KeyCode::Esc)); // back
    assert!(matches!(app.view_mode(), ViewMode::Board(_)));
}

#[test]
fn review_board_navigation() {
    let mut app = make_app();
    // Load some PRs
    app.update(Message::ReviewPrsLoaded(vec![
        make_review_pr(1, "alice", ReviewDecision::ReviewRequired),
        make_review_pr(2, "bob", ReviewDecision::ReviewRequired),
        make_review_pr(3, "carol", ReviewDecision::ChangesRequested),
    ]));
    app.handle_key(make_key(KeyCode::Tab)); // to review board
    assert_eq!(app.review_selection().unwrap().column(), 0);

    app.handle_key(make_key(KeyCode::Char('l'))); // move right
    assert_eq!(app.review_selection().unwrap().column(), 1);

    app.handle_key(make_key(KeyCode::Char('l'))); // move right
    assert_eq!(app.review_selection().unwrap().column(), 2);

    app.handle_key(make_key(KeyCode::Char('l'))); // move right
    assert_eq!(app.review_selection().unwrap().column(), 3);

    app.handle_key(make_key(KeyCode::Char('l'))); // clamp at 3
    assert_eq!(app.review_selection().unwrap().column(), 3);
}

#[test]
fn review_board_enter_toggles_detail() {
    let mut app = make_app();
    app.review_prs = vec![make_review_pr(1, "alice", ReviewDecision::ReviewRequired)];
    app.update(Message::SwitchToReviewBoard);
    assert!(!app.review_detail_visible);

    app.handle_key(make_key(KeyCode::Enter));
    assert!(app.review_detail_visible);

    app.handle_key(make_key(KeyCode::Enter));
    assert!(!app.review_detail_visible);
}

#[test]
fn review_board_p_opens_browser() {
    let mut app = make_app();
    app.review_prs = vec![make_review_pr(1, "alice", ReviewDecision::ReviewRequired)];
    app.update(Message::SwitchToReviewBoard);

    let cmds = app.handle_key(make_key(KeyCode::Char('p')));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::OpenInBrowser { .. })));
}

#[test]
fn review_board_renders_pr_titles() {
    let mut app = make_app();
    app.update(Message::ReviewPrsLoaded(vec![
        make_review_pr(42, "alice", ReviewDecision::ReviewRequired),
        make_review_pr(50, "bob", ReviewDecision::Approved),
    ]));
    app.update(Message::SwitchToReviewBoard);

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "Needs Review"), "Should show column header");
    assert!(buffer_contains(&buf, "PR 42"), "Should show PR title");
}

#[test]
fn review_board_renders_loading_state() {
    let mut app = make_app();
    // SwitchToReviewBoard triggers a fetch, so review_board_loading becomes true
    app.update(Message::SwitchToReviewBoard);
    assert!(app.review_board_loading());

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "Fetching reviews..."), "Should show loading text while fetching");
    assert!(!buffer_contains(&buf, "No PRs awaiting your review"), "Should not show empty-state text while loading");
}

#[test]
fn review_board_renders_empty_state_after_fetch() {
    let mut app = make_app();
    app.update(Message::SwitchToReviewBoard);
    app.update(Message::ReviewPrsLoaded(vec![]));
    assert!(!app.review_board_loading());

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "No PRs awaiting your review"), "Should show empty state after fetch with no results");
    assert!(!buffer_contains(&buf, "Fetching reviews..."), "Should not show loading text after fetch completes");
}

#[test]
fn review_board_renders_persistent_error() {
    let mut app = make_app();
    app.update(Message::SwitchToReviewBoard);
    app.update(Message::ReviewPrsFetchFailed("not authenticated".to_string()));
    assert_eq!(app.last_review_error(), Some("not authenticated"));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "not authenticated"), "Should show persistent error text in review board");
}

#[test]
fn review_prs_loaded_clears_last_error() {
    let mut app = make_app();
    app.update(Message::ReviewPrsFetchFailed("auth error".to_string()));
    assert!(app.last_review_error().is_some());

    app.update(Message::ReviewPrsLoaded(vec![]));
    assert!(app.last_review_error().is_none(), "Successful fetch should clear last error");
}

#[test]
fn handle_refresh_usage_stores_by_task_id() {
    use crate::models::TaskUsage;
    let mut app = make_app();
    let usage = vec![TaskUsage {
        task_id: TaskId(1),
        cost_usd: 0.42,
        input_tokens: 10_000,
        output_tokens: 2_000,
        cache_read_tokens: 500,
        cache_write_tokens: 100,
        updated_at: chrono::Utc::now(),
    }];
    app.update(Message::RefreshUsage(usage));
    assert!(app.usage.contains_key(&TaskId(1)));
    assert!((app.usage[&TaskId(1)].cost_usd - 0.42).abs() < 1e-9);
}

// --- Filter preset tests ---

#[test]
fn load_filter_preset_replaces_repo_filter() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.repo_filter.insert("/repo-a".to_string());

    let preset_repos: HashSet<String> = ["/repo-b".to_string()].into_iter().collect();
    app.filter_presets = vec![("backend".to_string(), preset_repos)];

    app.update(Message::LoadFilterPreset("backend".to_string()));
    assert!(app.repo_filter.contains("/repo-b"));
    assert!(!app.repo_filter.contains("/repo-a"));
}

#[test]
fn save_filter_preset_stores_and_persists() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.repo_filter.insert("/repo-a".to_string());
    app.input.mode = InputMode::RepoFilter;

    app.update(Message::StartSavePreset);
    assert_eq!(app.input.mode, InputMode::InputPresetName);

    let cmds = app.update(Message::SaveFilterPreset("frontend".to_string()));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert_eq!(app.filter_presets.len(), 1);
    assert_eq!(app.filter_presets[0].0, "frontend");
    assert!(app.filter_presets[0].1.contains("/repo-a"));
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistFilterPreset { .. })));
}

#[test]
fn save_filter_preset_empty_name_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.update(Message::SaveFilterPreset("  ".to_string()));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(app.filter_presets.is_empty());
}

#[test]
fn save_filter_preset_overwrites_existing() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    let old: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.filter_presets = vec![("frontend".to_string(), old)];

    app.repo_filter.insert("/repo-b".to_string());
    app.update(Message::SaveFilterPreset("frontend".to_string()));
    assert_eq!(app.filter_presets.len(), 1);
    assert!(app.filter_presets[0].1.contains("/repo-b"));
}

#[test]
fn delete_filter_preset_removes_and_returns_command() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.filter_presets = vec![("frontend".to_string(), repos)];
    app.input.mode = InputMode::ConfirmDeletePreset;

    let cmds = app.update(Message::DeleteFilterPreset("frontend".to_string()));
    assert!(app.filter_presets.is_empty());
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteFilterPreset(_))));
}

#[test]
fn cancel_preset_input_returns_to_repo_filter() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.input.buffer = "draft".to_string();
    app.update(Message::CancelPresetInput);
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(app.input.buffer.is_empty());
}

#[test]
fn filter_presets_loaded_sets_state() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.update(Message::FilterPresetsLoaded(vec![("frontend".to_string(), repos.clone())]));
    assert_eq!(app.filter_presets.len(), 1);
    assert_eq!(app.filter_presets[0].0, "frontend");
}

#[test]
fn load_filter_preset_unknown_name_is_noop() {
    let mut app = make_app();
    app.repo_filter.insert("/repo-a".to_string());
    app.update(Message::LoadFilterPreset("nonexistent".to_string()));
    assert!(app.repo_filter.contains("/repo-a"));
}

#[test]
fn load_filter_preset_skips_stale_paths() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    // Preset contains a path that no longer exists in repo_paths
    let preset_repos: HashSet<String> = ["/repo-a".to_string(), "/gone".to_string()].into_iter().collect();
    app.filter_presets = vec![("stale".to_string(), preset_repos)];

    app.update(Message::LoadFilterPreset("stale".to_string()));
    assert!(app.repo_filter.contains("/repo-a"));
    assert!(!app.repo_filter.contains("/gone"), "Stale path should be excluded");
}

#[test]
fn start_delete_preset_with_no_presets_is_noop() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.update(Message::StartDeletePreset);
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn repo_filter_s_key_starts_save_preset() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Char('s')));
    assert_eq!(app.input.mode, InputMode::InputPresetName);
}

#[test]
fn repo_filter_x_key_starts_delete_preset() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    app.filter_presets = vec![("test".to_string(), repos)];
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmDeletePreset);
}

#[test]
fn repo_filter_shift_a_loads_first_preset() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    let repos: HashSet<String> = ["/repo-b".to_string()].into_iter().collect();
    app.filter_presets = vec![("backend".to_string(), repos)];
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
    assert!(app.repo_filter.contains("/repo-b"));
    assert!(!app.repo_filter.contains("/repo-a"));
}

#[test]
fn input_preset_name_enter_saves() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string()];
    app.repo_filter.insert("/repo-a".to_string());
    app.input.mode = InputMode::InputPresetName;
    app.input.buffer = "mypreset".to_string();
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert_eq!(app.filter_presets.len(), 1);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistFilterPreset { .. })));
}

#[test]
fn input_preset_name_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.input.buffer = "draft".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn input_preset_name_typing_works() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('b')));
    assert_eq!(app.input.buffer, "ab");
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "a");
}

#[test]
fn confirm_delete_preset_letter_deletes() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    app.filter_presets = vec![("alpha".to_string(), repos)];
    app.input.mode = InputMode::ConfirmDeletePreset;
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
    assert!(app.filter_presets.is_empty());
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteFilterPreset(_))));
}

#[test]
fn confirm_delete_preset_esc_cancels() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    app.filter_presets = vec![("alpha".to_string(), repos)];
    app.input.mode = InputMode::ConfirmDeletePreset;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert_eq!(app.filter_presets.len(), 1);
}

#[test]
fn confirm_delete_preset_out_of_range_ignored() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    app.filter_presets = vec![("alpha".to_string(), repos)];
    app.input.mode = InputMode::ConfirmDeletePreset;
    app.handle_key(KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT));
    assert_eq!(app.input.mode, InputMode::ConfirmDeletePreset);
    assert_eq!(app.filter_presets.len(), 1);
}

// --- Overlay rendering tests ---

#[test]
fn repo_filter_overlay_shows_presets() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    let repos: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.filter_presets = vec![("frontend".to_string(), repos)];
    app.input.mode = InputMode::RepoFilter;

    let buf = render_to_buffer(&mut app, 80, 25);
    assert!(buffer_contains(&buf, "A"), "Expected preset letter A");
    assert!(buffer_contains(&buf, "frontend"), "Expected preset name 'frontend'");
}

#[test]
fn repo_filter_overlay_shows_name_input() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::InputPresetName;
    app.input.buffer = "myfilter".to_string();

    let buf = render_to_buffer(&mut app, 80, 25);
    assert!(buffer_contains(&buf, "Name:"), "Expected name input prompt");
    assert!(buffer_contains(&buf, "myfilter"), "Expected buffer content");
}

#[test]
fn repo_filter_overlay_shows_delete_help() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo-a".to_string()];
    let repos: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.filter_presets = vec![("test".to_string(), repos)];
    app.input.mode = InputMode::ConfirmDeletePreset;

    let buf = render_to_buffer(&mut app, 80, 25);
    assert!(buffer_contains(&buf, "delete preset"), "Expected delete help text");
}

// --- Epic batch wrap-up ---

fn make_review_subtask(id: i64, epic_id: i64, sort_order: i64) -> Task {
    let mut task = make_task(id, TaskStatus::Review);
    task.epic_id = Some(EpicId(epic_id));
    task.worktree = Some(format!("/repo/.worktrees/{id}-task-{id}"));
    task.sort_order = Some(sort_order);
    task
}

#[test]
fn w_key_on_epic_starts_epic_wrap_up() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 1),
    ], TEST_TIMEOUT);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Review;
    app.epics = vec![epic];
    // Epic is in Review column (column 2)
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);

    app.handle_key(make_key(KeyCode::Char('W')));

    assert!(matches!(app.input.mode, InputMode::ConfirmEpicWrapUp(_)));
}

#[test]
fn epic_wrap_up_with_review_tasks_enters_confirm() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 1),
        make_review_subtask(2, 10, 2),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];

    app.update(Message::StartEpicWrapUp(EpicId(10)));

    assert!(matches!(app.input.mode, InputMode::ConfirmEpicWrapUp(EpicId(10))));
}

#[test]
fn epic_wrap_up_without_review_tasks_shows_info() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.epic_id = Some(EpicId(10));
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];

    app.update(Message::StartEpicWrapUp(EpicId(10)));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.as_ref().unwrap().contains("No review tasks"));
}

#[test]
fn epic_wrap_up_rebase_creates_queue_and_emits_first_finish() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 2),
        make_review_subtask(2, 10, 1),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(10));

    let cmds = app.update(Message::EpicWrapUpRebase);

    assert_eq!(app.input.mode, InputMode::Normal);
    let queue = app.merge_queue.as_ref().expect("merge queue should exist");
    assert_eq!(queue.action, MergeAction::Rebase);
    // Task 2 has sort_order 1, so it comes first
    assert_eq!(queue.task_ids, vec![TaskId(2), TaskId(1)]);
    assert_eq!(queue.current, Some(TaskId(2)));
    assert!(cmds.iter().any(|c| matches!(c, Command::Finish { id, .. } if *id == TaskId(2))));
}

#[test]
fn epic_wrap_up_finish_complete_advances_queue() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 2),
        make_review_subtask(2, 10, 1),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(10));
    app.update(Message::EpicWrapUpRebase);

    // First task completes
    let cmds = app.update(Message::FinishComplete(TaskId(2)));

    let queue = app.merge_queue.as_ref().expect("queue should still exist");
    assert_eq!(queue.completed, 1);
    assert_eq!(queue.current, Some(TaskId(1)));
    assert!(cmds.iter().any(|c| matches!(c, Command::Finish { id, .. } if *id == TaskId(1))));
}

#[test]
fn epic_wrap_up_all_complete_clears_queue() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 2),
        make_review_subtask(2, 10, 1),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(10));
    app.update(Message::EpicWrapUpRebase);

    app.update(Message::FinishComplete(TaskId(2)));
    app.update(Message::FinishComplete(TaskId(1)));

    assert!(app.merge_queue.is_none(), "queue should be cleared after all tasks complete");
}

#[test]
fn epic_wrap_up_finish_failed_pauses_queue() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 2),
        make_review_subtask(2, 10, 1),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(10));
    app.update(Message::EpicWrapUpRebase);

    app.update(Message::FinishFailed {
        id: TaskId(2),
        error: "rebase conflict".to_string(),
        is_conflict: true,
    });

    let queue = app.merge_queue.as_ref().expect("queue should still exist");
    assert_eq!(queue.failed, Some(TaskId(2)));
    assert!(queue.current.is_none());
}

#[test]
fn epic_wrap_up_cancel_clears_queue() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 1),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.merge_queue = Some(MergeQueue {
        epic_id: EpicId(10),
        action: MergeAction::Rebase,
        task_ids: vec![TaskId(1)],
        completed: 0,
        current: Some(TaskId(1)),
        failed: None,
    });

    app.update(Message::CancelMergeQueue);

    assert!(app.merge_queue.is_none());
}

#[test]
fn epic_wrap_up_pr_mode_advances_on_pr_created() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 2),
        make_review_subtask(2, 10, 1),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(10));
    app.update(Message::EpicWrapUpPr);

    let cmds = app.update(Message::PrCreated {
        id: TaskId(2),
        pr_url: "https://github.com/org/repo/pull/1".to_string(),
    });

    let queue = app.merge_queue.as_ref().expect("queue should still exist");
    assert_eq!(queue.completed, 1);
    assert_eq!(queue.current, Some(TaskId(1)));
    assert!(cmds.iter().any(|c| matches!(c, Command::CreatePr { id, .. } if *id == TaskId(1))));
}

// ---------------------------------------------------------------------------
// SubStatus stale/crashed detection, escalation, and recovery
// ---------------------------------------------------------------------------

#[test]
fn stale_detection_sets_substatus_and_persists() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("win-3".to_string());

    let cmds = app.update(Message::StaleAgent(TaskId(3)));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Stale);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.id == TaskId(3))));
}

#[test]
fn crashed_detection_sets_substatus_and_persists() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("win-3".to_string());

    let cmds = app.update(Message::AgentCrashed(TaskId(3)));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Crashed);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.id == TaskId(3))));
}

#[test]
fn stale_does_not_overwrite_crashed() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("win-3".to_string());
    app.tasks[0].sub_status = SubStatus::Crashed;

    let cmds = app.update(Message::StaleAgent(TaskId(3)));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Crashed); // unchanged
    assert!(cmds.is_empty()); // no persist needed
}

#[test]
fn stale_skips_non_running_task() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Backlog),
    ], TEST_TIMEOUT);

    let cmds = app.update(Message::StaleAgent(TaskId(3)));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::None); // unchanged
    assert!(cmds.is_empty());
}

#[test]
fn crashed_skips_non_running_task() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Review),
    ], TEST_TIMEOUT);

    let cmds = app.update(Message::AgentCrashed(TaskId(3)));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::AwaitingReview); // unchanged
    assert!(cmds.is_empty());
}

#[test]
fn recovery_from_stale_resets_substatus_to_active() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].sub_status = SubStatus::Stale;
    app.tasks[0].tmux_window = Some("win-3".to_string());

    let cmds = app.update(Message::TmuxOutput { id: TaskId(3), output: "new output".to_string(), activity_ts: 1 });
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Active);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.id == TaskId(3))));
}

#[test]
fn recovery_from_crashed_resets_substatus_to_active() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].sub_status = SubStatus::Crashed;
    app.tasks[0].tmux_window = Some("win-3".to_string());

    let cmds = app.update(Message::TmuxOutput { id: TaskId(3), output: "new output".to_string(), activity_ts: 1 });
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Active);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.id == TaskId(3))));
}

#[test]
fn active_task_output_does_not_emit_persist() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].sub_status = SubStatus::Active;
    app.tasks[0].tmux_window = Some("win-3".to_string());

    let cmds = app.update(Message::TmuxOutput { id: TaskId(3), output: "output".to_string(), activity_ts: 1 });
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Active); // unchanged
    // No PersistTask since sub_status didn't change
    assert!(!cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn stale_notification_sent_when_enabled() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("win-3".to_string());
    app.set_notifications_enabled(true);

    let cmds = app.update(Message::StaleAgent(TaskId(3)));
    assert!(cmds.iter().any(|c| matches!(c, Command::SendNotification { urgent: false, .. })));
}

#[test]
fn stale_notification_not_sent_when_disabled() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("win-3".to_string());
    app.set_notifications_enabled(false);

    let cmds = app.update(Message::StaleAgent(TaskId(3)));
    assert!(!cmds.iter().any(|c| matches!(c, Command::SendNotification { .. })));
}

#[test]
fn crashed_notification_sent_urgent_when_enabled() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("win-3".to_string());
    app.set_notifications_enabled(true);

    let cmds = app.update(Message::AgentCrashed(TaskId(3)));
    assert!(cmds.iter().any(|c| matches!(c, Command::SendNotification { urgent: true, .. })));
}

#[test]
fn crashed_notification_not_sent_when_disabled() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("win-3".to_string());
    app.set_notifications_enabled(false);

    let cmds = app.update(Message::AgentCrashed(TaskId(3)));
    assert!(!cmds.iter().any(|c| matches!(c, Command::SendNotification { .. })));
}

#[test]
fn tick_skips_already_stale_tasks() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("win-3".to_string());
    app.tasks[0].sub_status = SubStatus::Stale;
    app.agents.last_output_change.insert(TaskId(3), Instant::now() - Duration::from_secs(301));

    let cmds = app.update(Message::Tick);
    // Tick should NOT re-emit PersistTask for already-stale tasks
    // (only CaptureTmux and RefreshFromDb expected)
    assert!(!cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn tick_skips_already_crashed_tasks() {
    let mut app = App::new(vec![
        make_task(3, TaskStatus::Running),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("win-3".to_string());
    app.tasks[0].sub_status = SubStatus::Crashed;
    app.agents.last_output_change.insert(TaskId(3), Instant::now() - Duration::from_secs(301));

    let cmds = app.update(Message::Tick);
    assert!(!cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn move_task_forward_resets_substatus() {
    let mut app = make_app();
    let id = TaskId(3); // Running
    app.find_task_mut(id).unwrap().sub_status = SubStatus::Stale;
    app.update(Message::MoveTask { id, direction: MoveDirection::Forward });
    let task = app.find_task(id).unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::AwaitingReview);
}

#[test]
fn move_task_backward_resets_substatus() {
    let mut app = make_app();
    let id = TaskId(3); // Running
    app.update(Message::MoveTask { id, direction: MoveDirection::Backward });
    let task = app.find_task(id).unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert_eq!(task.sub_status, SubStatus::None);
}

#[test]
fn render_shows_subcolumn_headers() {
    // make_app() has one Running task (SubStatus::Active) → Running column shows "── active" header
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Running),
        {
            let mut t = make_task(2, TaskStatus::Running);
            t.sub_status = SubStatus::Stale;
            t
        },
    ], TEST_TIMEOUT);
    let buf = render_to_buffer(&mut app, 160, 30);
    assert!(buffer_contains(&buf, "active"), "section header 'active' not found");
    assert!(buffer_contains(&buf, "stale"), "section header 'stale' not found");
}

#[test]
fn render_shows_parent_status_headers() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 160, 30);
    assert!(buffer_contains(&buf, "backlog"), "parent header 'backlog' not found");
    assert!(buffer_contains(&buf, "running"), "parent header 'running' not found");
    assert!(buffer_contains(&buf, "review"), "parent header 'review' not found");
    assert!(buffer_contains(&buf, "done"), "parent header 'done' not found");
}

#[test]
fn render_detail_shows_sub_status() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Active;
    let mut app = App::new(vec![task], TEST_TIMEOUT);
    // Navigate to the Active visual column (index 1)
    app.update(Message::NavigateColumn(1));
    // Open the detail panel
    app.update(Message::ToggleDetail);
    let buf = render_to_buffer(&mut app, 160, 30);
    assert!(buffer_contains(&buf, "(active)"), "detail panel should show sub-status '(active)'");
}

// ---------------------------------------------------------------------------
// PrReviewState message handling
// ---------------------------------------------------------------------------

#[test]
fn pr_review_state_updates_substatus() {
    let mut app = make_app();
    let id = TaskId(3);
    app.find_task_mut(id).unwrap().status = TaskStatus::Review;
    app.find_task_mut(id).unwrap().sub_status = SubStatus::AwaitingReview;
    let cmds = app.update(Message::PrReviewState {
        id,
        review_decision: Some(dispatch::PrReviewDecision::Approved),
    });
    let task = app.find_task(id).unwrap();
    assert_eq!(task.sub_status, SubStatus::Approved);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn pr_review_state_noop_when_unchanged() {
    let mut app = make_app();
    let id = TaskId(3);
    app.find_task_mut(id).unwrap().status = TaskStatus::Review;
    app.find_task_mut(id).unwrap().sub_status = SubStatus::AwaitingReview;
    let cmds = app.update(Message::PrReviewState {
        id,
        review_decision: None, // maps to AwaitingReview
    });
    assert!(cmds.is_empty()); // no change, no persist
}

#[test]
fn pr_review_state_changes_requested() {
    let mut app = make_app();
    let id = TaskId(3);
    app.find_task_mut(id).unwrap().status = TaskStatus::Review;
    app.find_task_mut(id).unwrap().sub_status = SubStatus::AwaitingReview;
    let cmds = app.update(Message::PrReviewState {
        id,
        review_decision: Some(dispatch::PrReviewDecision::ChangesRequested),
    });
    let task = app.find_task(id).unwrap();
    assert_eq!(task.sub_status, SubStatus::ChangesRequested);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn pr_review_state_ignores_non_review_task() {
    let mut app = make_app();
    let id = TaskId(3);
    // Task 3 is Running by default in make_app
    assert_eq!(app.find_task(id).unwrap().status, TaskStatus::Running);
    let cmds = app.update(Message::PrReviewState {
        id,
        review_decision: Some(dispatch::PrReviewDecision::Approved),
    });
    assert!(cmds.is_empty());
    // sub_status should not have changed
    assert_ne!(app.find_task(id).unwrap().sub_status, SubStatus::Approved);
}

// =====================================================================
// Input handler tests (tui/input.rs)
// =====================================================================

#[test]
fn handle_key_dismisses_error_popup() {
    let mut app = make_app();
    app.error_popup = Some("something went wrong".to_string());
    let cmds = app.handle_key(make_key(KeyCode::Char('q')));
    assert!(app.error_popup.is_none());
    assert!(cmds.is_empty());
}

#[test]
fn handle_key_normal_navigation() {
    let mut app = make_app();
    // Start at column 0, row 0
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    // 'l' moves right
    app.handle_key(make_key(KeyCode::Char('l')));
    assert_eq!(app.selection().column(), 1);

    // 'h' moves left
    app.handle_key(make_key(KeyCode::Char('h')));
    assert_eq!(app.selection().column(), 0);

    // 'j' moves down
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.selection().row(0), 1);

    // 'k' moves up
    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.selection().row(0), 0);
}

#[test]
fn handle_key_normal_quit() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('q')));
    assert!(app.should_quit);
}

#[test]
fn handle_key_normal_new_task() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(*app.mode(), InputMode::InputTitle);
}

#[test]
fn handle_key_normal_new_epic() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('E')));
    assert_eq!(*app.mode(), InputMode::InputEpicTitle);
}

#[test]
fn handle_key_normal_toggle_help() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('?')));
    assert_eq!(*app.mode(), InputMode::Help);
}

#[test]
fn handle_key_help_dismiss() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;

    // '?' toggles help off
    app.handle_key(make_key(KeyCode::Char('?')));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_help_esc_dismiss() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_text_input_char_and_backspace() {
    let mut app = make_app();
    // Enter title input mode
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(*app.mode(), InputMode::InputTitle);

    // Type characters
    app.handle_key(make_key(KeyCode::Char('H')));
    app.handle_key(make_key(KeyCode::Char('i')));
    assert_eq!(app.input.buffer, "Hi");

    // Backspace removes last char
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "H");
}

#[test]
fn handle_key_text_input_esc_cancels() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(*app.mode(), InputMode::InputTitle);

    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_text_input_enter_advances_to_tag() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    app.handle_key(make_key(KeyCode::Char('T')));
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(*app.mode(), InputMode::InputTag);
}

#[test]
fn handle_key_confirm_archive_yes() {
    let mut app = make_app();
    // Select task 1 (backlog)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);
    app.input.mode = InputMode::ConfirmArchive;

    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(*app.mode(), InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.status == TaskStatus::Archived)));
}

#[test]
fn handle_key_confirm_archive_cancel() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmArchive;

    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_retry_resume() {
    let mut app = make_app();
    let mut task = make_task(10, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/10-test".to_string());
    task.tmux_window = Some("main:10-test".to_string());
    app.tasks.push(task);
    app.input.mode = InputMode::ConfirmRetry(TaskId(10));

    let cmds = app.handle_key(make_key(KeyCode::Char('r')));
    // Should produce KillTmuxWindow + Resume
    assert!(cmds.iter().any(|c| matches!(c, Command::KillTmuxWindow { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::Resume { .. })));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_retry_fresh() {
    let mut app = make_app();
    let mut task = make_task(10, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/10-test".to_string());
    task.tmux_window = Some("main:10-test".to_string());
    app.tasks.push(task);
    app.input.mode = InputMode::ConfirmRetry(TaskId(10));

    let cmds = app.handle_key(make_key(KeyCode::Char('f')));
    // Should produce Cleanup + Dispatch
    assert!(cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::Dispatch { .. })));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_retry_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmRetry(TaskId(10));
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_quick_dispatch_digit_selects() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::QuickDispatch;

    let cmds = app.handle_key(make_key(KeyCode::Char('1')));
    // Should produce a QuickDispatch command
    assert!(cmds.iter().any(|c| matches!(c, Command::QuickDispatch { .. })));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_quick_dispatch_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::QuickDispatch;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_done_yes() {
    let mut app = make_app();
    // Move task 3 (Running) to Review so ConfirmDone makes sense
    let task_3 = app.tasks.iter_mut().find(|t| t.id == TaskId(3)).unwrap();
    task_3.status = TaskStatus::Review;
    app.input.mode = InputMode::ConfirmDone(TaskId(3));

    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(*app.mode(), InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.id == TaskId(3) && t.status == TaskStatus::Done)));
}

#[test]
fn handle_key_confirm_done_cancel() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDone(TaskId(3));
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_wrap_up_rebase() {
    let mut app = make_app();
    let mut task = make_task(10, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/10-test".to_string());
    task.tmux_window = Some("main:10-test".to_string());
    app.tasks.push(task);
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(10));

    let cmds = app.handle_key(make_key(KeyCode::Char('r')));
    assert!(cmds.iter().any(|c| matches!(c, Command::Finish { id, .. } if *id == TaskId(10))));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_wrap_up_pr() {
    let mut app = make_app();
    let mut task = make_task(10, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/10-test".to_string());
    task.tmux_window = Some("main:10-test".to_string());
    app.tasks.push(task);
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(10));

    let cmds = app.handle_key(make_key(KeyCode::Char('p')));
    assert!(cmds.iter().any(|c| matches!(c, Command::CreatePr { id, .. } if *id == TaskId(10))));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_wrap_up_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(10));
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_tag_selects_bug() {
    let mut app = make_app();
    // Tag comes right after title, before description/repo
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    let cmds = app.handle_key(make_key(KeyCode::Char('b')));
    assert!(cmds.is_empty());
    assert_eq!(*app.mode(), InputMode::InputDescription);
    assert_eq!(app.input.task_draft.as_ref().unwrap().tag, Some("bug".to_string()));
}

#[test]
fn handle_key_tag_skip_with_enter() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert!(cmds.is_empty());
    assert_eq!(*app.mode(), InputMode::InputDescription);
    assert_eq!(app.input.task_draft.as_ref().unwrap().tag, None);
}

#[test]
fn handle_key_tag_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn render_input_form_shows_during_input_tag() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "My task".to_string(),
        ..Default::default()
    });

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "New Task"), "form overlay title should be visible");
    assert!(buffer_contains(&buf, "My task"), "draft title should be shown as completed");
    assert!(buffer_contains(&buf, "(b)ug"), "tag options should be visible");
}

#[test]
fn handle_key_repo_filter_toggle() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('1')));
    assert!(app.repo_filter.contains("/repo"));
}

#[test]
fn handle_key_repo_filter_close_enter() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_repo_filter_close_esc() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_repo_filter_close_q() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Char('q')));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_normal_dispatch_backlog_task() {
    let mut app = make_app();
    // Select task 1 (backlog)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.iter().any(|c| matches!(c, Command::Dispatch { .. })));
}

#[test]
fn handle_key_normal_dispatch_running_task_with_window_shows_info() {
    let mut app = make_app();
    // Select running task (column 1)
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    // Give running task a window
    let task_3 = app.tasks.iter_mut().find(|t| t.id == TaskId(3)).unwrap();
    task_3.tmux_window = Some("main:task-3".to_string());

    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    // Should just show status info, no dispatch
    assert!(cmds.is_empty());
}

#[test]
fn handle_key_normal_toggle_archive() {
    let mut app = make_app();
    assert!(!app.archive.visible);
    app.handle_key(make_key(KeyCode::Char('H')));
    assert!(app.archive.visible);
}

#[test]
fn handle_key_normal_enter_toggles_detail() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);
    assert!(!app.detail_visible);
    app.handle_key(make_key(KeyCode::Enter));
    assert!(app.detail_visible);
    app.handle_key(make_key(KeyCode::Enter));
    assert!(!app.detail_visible);
}

#[test]
fn handle_key_normal_jump_to_tmux() {
    let mut app = make_app();
    // Give task 3 (running) a tmux window
    let task = app.tasks.iter_mut().find(|t| t.id == TaskId(3)).unwrap();
    task.tmux_window = Some("main:task-3".to_string());
    // Select running column
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(cmds.iter().any(|c| matches!(c, Command::JumpToTmux { window } if window == "main:task-3")));
}

#[test]
fn handle_key_normal_open_pr_url() {
    let mut app = make_app();
    let task = app.tasks.iter_mut().find(|t| t.id == TaskId(1)).unwrap();
    task.pr_url = Some("https://github.com/example/repo/pull/42".to_string());
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('p')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::OpenInBrowser { url } if url == "https://github.com/example/repo/pull/42"
    )));
}

#[test]
fn handle_key_normal_open_pr_url_missing() {
    let mut app = make_app();
    // task 1 has no pr_url by default
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('p')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("No PR URL"));
}

#[test]
fn handle_key_normal_tab_switches_to_review_board() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Tab));
    assert!(matches!(app.view_mode, ViewMode::ReviewBoard { .. }));
}

#[test]
fn handle_key_review_board_tab_switches_back() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Tab)); // to review board
    assert!(matches!(app.view_mode, ViewMode::ReviewBoard { .. }));
    app.handle_key(make_key(KeyCode::Tab)); // back to task board
    assert!(matches!(app.view_mode, ViewMode::Board(_)));
}

#[test]
fn handle_key_epic_text_input_char_and_enter() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('E'))); // start epic creation
    assert_eq!(*app.mode(), InputMode::InputEpicTitle);

    app.handle_key(make_key(KeyCode::Char('X')));
    assert_eq!(app.input.buffer, "X");

    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(*app.mode(), InputMode::InputEpicDescription);
}

#[test]
fn handle_key_epic_text_input_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::InputEpicTitle;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_input_preset_name_enter_saves() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::InputPresetName;
    app.input.buffer = "my-preset".to_string();

    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistFilterPreset { .. })));
}

#[test]
fn handle_key_input_preset_name_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::RepoFilter);
}

#[test]
fn handle_key_confirm_delete_preset_selects() {
    let mut app = make_app();
    app.filter_presets = vec![("preset-a".to_string(), std::collections::HashSet::new())];
    app.input.mode = InputMode::ConfirmDeletePreset;

    let cmds = app.handle_key(make_key(KeyCode::Char('A')));
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteFilterPreset(_))));
}

#[test]
fn handle_key_confirm_delete_preset_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDeletePreset;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::RepoFilter);
}

// ---------------------------------------------------------------------------
// Epic selection tests
// ---------------------------------------------------------------------------

#[test]
fn space_toggles_epic_selection() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    // Epic is at row 0 in Backlog column (no standalone tasks)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.selected_epics.contains(&EpicId(10)));
}

#[test]
fn space_on_epic_toggle_off() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    // Select
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.selected_epics.contains(&EpicId(10)));

    // Deselect
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(!app.selected_epics.contains(&EpicId(10)));
}

#[test]
fn space_on_empty_column_no_epics_is_noop() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    // Navigate to Review column (empty)
    app.update(Message::NavigateColumn(2));
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.selected_epics.is_empty());
    assert!(app.selected_tasks.is_empty());
}

#[test]
fn select_all_column_includes_epics() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];

    app.update(Message::SelectAllColumn);
    assert!(app.selected_tasks.contains(&TaskId(1)));
    assert!(app.selected_epics.contains(&EpicId(10)));
}

#[test]
fn select_all_deselects_all_including_epics() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];

    // Select all
    app.update(Message::SelectAllColumn);
    assert_eq!(app.selected_tasks.len(), 1);
    assert_eq!(app.selected_epics.len(), 1);

    // Deselect all
    app.update(Message::SelectAllColumn);
    assert!(app.selected_tasks.is_empty());
    assert!(app.selected_epics.is_empty());
}

#[test]
fn select_all_column_with_only_epics() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10), make_epic(20)];

    app.update(Message::SelectAllColumn);
    assert!(app.selected_tasks.is_empty());
    assert_eq!(app.selected_epics.len(), 2);
    assert!(app.selected_epics.contains(&EpicId(10)));
    assert!(app.selected_epics.contains(&EpicId(20)));
}

#[test]
fn esc_clears_epic_selection() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.update(Message::ToggleSelectEpic(EpicId(10)));
    assert_eq!(app.selected_epics.len(), 1);

    app.handle_key(make_key(KeyCode::Esc));
    assert!(app.selected_epics.is_empty());
}

#[test]
fn esc_clears_mixed_selection() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelectEpic(EpicId(10)));

    app.handle_key(make_key(KeyCode::Esc));
    assert!(app.selected_tasks.is_empty());
    assert!(app.selected_epics.is_empty());
}

#[test]
fn batch_archive_selected_epics() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10), make_epic(20)];

    let cmds = app.update(Message::BatchArchiveEpics(vec![EpicId(10), EpicId(20)]));
    assert!(app.epics.is_empty(), "Both epics should be removed");
    assert!(!cmds.is_empty(), "Should emit commands");
}

#[test]
fn x_key_with_epic_selection_shows_count_in_confirm() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10), make_epic(20)];
    app.update(Message::ToggleSelectEpic(EpicId(10)));
    app.update(Message::ToggleSelectEpic(EpicId(20)));

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmArchive);
    assert_eq!(app.status_message.as_deref(), Some("Archive 2 items? (y/n)"));
}

#[test]
fn batch_archive_mixed_tasks_and_epics() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelectEpic(EpicId(10)));

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmArchive);
    assert_eq!(app.status_message.as_deref(), Some("Archive 2 items? (y/n)"));

    // Confirm
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Archived);
    assert!(app.epics.is_empty(), "Epic should be removed");
    assert!(app.selected_tasks.is_empty());
    assert!(app.selected_epics.is_empty());
    assert!(!cmds.is_empty());
}

#[test]
fn confirm_archive_y_archives_selected_epics() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.update(Message::ToggleSelectEpic(EpicId(10)));
    app.input.mode = InputMode::ConfirmArchive;

    app.handle_key(make_key(KeyCode::Char('y')));
    assert!(app.epics.is_empty());
    assert!(app.selected_epics.is_empty());
}

#[test]
fn m_on_epic_moves_status_forward() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    // Cursor on Backlog column, row 0 (the epic)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert_eq!(app.epics[0].status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistEpic { id: EpicId(10), status: Some(TaskStatus::Running), .. })));
}

#[test]
fn m_with_mixed_selection_moves_tasks_only() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelectEpic(EpicId(10)));
    // Cursor on the task (row 1) so 'm' triggers batch move, not epic move
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 1);

    app.handle_key(make_key(KeyCode::Char('m')));
    // Task should move forward
    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Running);
}

#[test]
fn render_selected_epic_shows_star_prefix() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.update(Message::ToggleSelectEpic(EpicId(10)));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "* "), "Selected epic should show * prefix");
    assert!(buffer_contains(&buf, "Epic 10"), "Epic title should be visible");
}

#[test]
fn render_unselected_epic_no_star() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "Epic 10"), "Epic title should be visible");
    // The epic renders with "  " prefix (2 spaces), not "* "
    assert!(!buffer_contains(&buf, "* "), "Unselected epic should not show * prefix");
}

#[test]
fn render_batch_hints_with_epic_selection() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.update(Message::ToggleSelectEpic(EpicId(10)));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "1 selected"), "Should show selection count");
    assert!(buffer_contains(&buf, "archive"), "Should show archive hint");
}

#[test]
fn render_column_header_checked_with_epics() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];

    // Select both the task and the epic
    app.update(Message::SelectAllColumn);
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "[x]"), "Checkbox should be checked when all items selected");
}

#[test]
fn refresh_epics_prunes_stale_epic_selections() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.epics = vec![make_epic(10)];
    app.update(Message::ToggleSelectEpic(EpicId(10)));
    app.update(Message::ToggleSelectEpic(EpicId(99))); // non-existent

    // Refresh with only epic 10
    app.update(Message::RefreshEpics(vec![make_epic(10)]));
    assert!(app.selected_epics.contains(&EpicId(10)));
    assert!(!app.selected_epics.contains(&EpicId(99)));
}

#[test]
fn detach_tmux_single_sets_confirm_mode() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-1".to_string());

    app.update(Message::DetachTmux(TaskId(1)));

    assert!(
        matches!(&app.input.mode, InputMode::ConfirmDetachTmux(ids) if ids == &[TaskId(1)]),
        "Expected ConfirmDetachTmux([1]), got {:?}", app.input.mode
    );
    assert!(app.status_message.is_some());
}

#[test]
fn confirm_detach_tmux_clears_window() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-1".to_string());
    app.tasks[0].sub_status = SubStatus::Stale;
    app.agents.tmux_outputs.insert(TaskId(1), "some output".to_string());

    app.update(Message::DetachTmux(TaskId(1)));
    let cmds = app.update(Message::ConfirmDetachTmux);

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.tasks[0].tmux_window.is_none(), "tmux_window should be cleared");
    assert_ne!(app.find_task(TaskId(1)).unwrap().sub_status, SubStatus::Stale, "stale tracking should be cleared");
    assert!(!app.agents.tmux_outputs.contains_key(&TaskId(1)), "tmux output should be cleared");
    assert!(
        cmds.iter().any(|c| matches!(c, Command::KillTmuxWindow { window } if window == "task-1")),
        "should emit KillTmuxWindow for task-1"
    );
    assert!(
        cmds.iter().any(|c| matches!(c, Command::PersistTask(_))),
        "should emit PersistTask"
    );
}

#[test]
fn detach_tmux_noop_on_task_without_window() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)], TEST_TIMEOUT);
    // tmux_window is None by default from make_task

    let cmds = app.update(Message::DetachTmux(TaskId(1)));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty(), "should produce no commands");
}

#[test]
fn batch_detach_tmux() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
        make_task(2, TaskStatus::Review),
    ], TEST_TIMEOUT);
    app.tasks[0].tmux_window = Some("task-1".to_string());
    app.tasks[1].tmux_window = Some("task-2".to_string());

    app.update(Message::BatchDetachTmux(vec![TaskId(1), TaskId(2)]));
    let cmds = app.update(Message::ConfirmDetachTmux);

    assert!(app.tasks[0].tmux_window.is_none(), "task 1 window should be cleared");
    assert!(app.tasks[1].tmux_window.is_none(), "task 2 window should be cleared");

    let kill_count = cmds.iter()
        .filter(|c| matches!(c, Command::KillTmuxWindow { .. }))
        .count();
    assert_eq!(kill_count, 2, "should kill 2 windows");

    let persist_count = cmds.iter()
        .filter(|c| matches!(c, Command::PersistTask(_)))
        .count();
    assert_eq!(persist_count, 2, "should persist 2 tasks");
}

// --- Repo cursor navigation ---

#[test]
fn quick_dispatch_j_moves_cursor_down() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 0;
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.input.repo_cursor, 1);
}

#[test]
fn move_repo_cursor_down_wraps() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/a".to_string(), "/b".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 1; // last
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.input.repo_cursor, 0, "should wrap to first");
}

#[test]
fn move_repo_cursor_up_wraps() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    app.repo_paths = vec!["/a".to_string(), "/b".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 0; // first
    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.input.repo_cursor, 1, "should wrap to last");
}

#[test]
fn quick_dispatch_enter_selects_cursor_repo() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string(), "/repo3".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 2; // third repo
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::QuickDispatch { ref draft, epic_id: None } if draft.repo_path == "/repo3"));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn review_board_d_dispatches_review_agent() {
    let mut app = make_app();
    // The default make_task uses repo_path "/repo"
    // Create a PR where repo matches the task's repo_path directory name
    let mut pr = make_review_pr(42, "alice", ReviewDecision::ReviewRequired);
    pr.repo = "org/repo".to_string();
    pr.head_ref = "fix-bug".to_string();
    app.review_prs = vec![pr];
    app.update(Message::SwitchToReviewBoard);

    let cmds = app.handle_key(KeyEvent::from(KeyCode::Char('d')));
    assert!(cmds.iter().any(|c| matches!(c, Command::DispatchReviewAgent { .. })));
}

#[test]
fn review_board_d_no_repo_shows_error() {
    let mut app = App::new(vec![], TEST_TIMEOUT);
    let pr = make_review_pr(42, "alice", ReviewDecision::ReviewRequired);
    app.review_prs = vec![pr];
    app.update(Message::SwitchToReviewBoard);

    let cmds = app.handle_key(KeyEvent::from(KeyCode::Char('d')));
    assert!(cmds.is_empty()); // no command, just status message
    assert!(app.status_message.as_deref().unwrap().contains("No local repo"));
}

#[test]
fn review_agent_dispatched_sets_status() {
    let mut app = make_app();
    app.update(Message::SwitchToReviewBoard);
    let cmds = app.update(Message::ReviewAgentDispatched {
        repo: "org/my-repo".to_string(),
        number: 99,
        tmux_window: "review-my-repo-99".to_string(),
    });
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("my-repo#99"));
}

#[test]
fn review_agent_failed_sets_status() {
    let mut app = make_app();
    app.update(Message::SwitchToReviewBoard);
    let cmds = app.update(Message::ReviewAgentFailed {
        error: "git fetch failed".to_string(),
    });
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("git fetch failed"));
}

#[test]
fn clamp_review_selection_clamps_approved_column() {
    // The Approved column is at index 3. Load a PR into it, set the row
    // selection beyond the end, clear the PR list, and verify the selection
    // is clamped to 0.
    let mut app = make_app();

    // Switch to the review board so that review_selection_mut() returns Some.
    app.update(Message::SwitchToReviewBoard);

    // Manually push a PR into the Approved column (index 3).
    app.review_prs = vec![make_review_pr(1, "alice", ReviewDecision::Approved)];

    // Set the row selection for the Approved column to an out-of-bounds value.
    if let Some(sel) = app.review_selection_mut() {
        sel.selected_row[3] = 5;
    }

    // Now remove all PRs and trigger a clamp via ReviewPrsLoaded with an
    // empty list (which calls clamp_review_selection internally).
    app.update(Message::ReviewPrsLoaded(vec![]));

    // The Approved column selection must have been clamped to 0.
    let row = app.review_selection().unwrap().selected_row[3];
    assert_eq!(row, 0, "Approved column (index 3) selection was not clamped");
}

#[test]
fn review_repo_filter_hides_prs() {
    let mut app = make_app();
    let mut pr1 = make_review_pr(1, "alice", ReviewDecision::ReviewRequired);
    pr1.repo = "org/repo-a".to_string();
    let mut pr2 = make_review_pr(2, "bob", ReviewDecision::ReviewRequired);
    pr2.repo = "org/repo-b".to_string();
    app.review_prs = vec![pr1, pr2];
    app.update(Message::SwitchToReviewBoard);

    // No filter — both visible
    assert_eq!(app.filtered_review_prs().len(), 2);

    // Filter to repo-a only
    app.review_repo_filter.insert("org/repo-a".to_string());
    assert_eq!(app.filtered_review_prs().len(), 1);
    assert_eq!(app.filtered_review_prs()[0].repo, "org/repo-a");
}

#[test]
fn review_repo_filter_f_opens_filter() {
    let mut app = make_app();
    app.review_prs = vec![make_review_pr(1, "alice", ReviewDecision::ReviewRequired)];
    app.update(Message::SwitchToReviewBoard);

    app.handle_key(KeyEvent::from(KeyCode::Char('f')));
    assert_eq!(app.input.mode, InputMode::ReviewRepoFilter);
}

#[test]
fn review_repo_filter_esc_closes() {
    let mut app = make_app();
    app.review_prs = vec![make_review_pr(1, "alice", ReviewDecision::ReviewRequired)];
    app.update(Message::SwitchToReviewBoard);
    app.update(Message::StartReviewRepoFilter);
    assert_eq!(app.input.mode, InputMode::ReviewRepoFilter);

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn repo_cursor_resets_on_quick_dispatch_entry() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], TEST_TIMEOUT);
    app.repo_paths = vec!["/a".to_string(), "/b".to_string()];
    app.input.repo_cursor = 1;
    app.update(Message::StartQuickDispatchSelection);
    assert_eq!(app.input.repo_cursor, 0, "cursor should reset to 0 on mode entry");
}

#[test]
fn repo_filter_j_moves_cursor_down() {
    let mut app = make_app();
    app.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 0;
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.input.repo_cursor, 1);
}

#[test]
fn repo_filter_space_toggles_cursor_repo() {
    let mut app = make_app();
    app.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 1; // pointing at /repo-b
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.repo_filter.contains("/repo-b"), "cursor repo should be toggled");
    assert!(!app.repo_filter.contains("/repo-a"));
}

#[test]
fn review_repo_filter_toggle_all() {
    let mut app = make_app();
    let mut pr1 = make_review_pr(1, "alice", ReviewDecision::ReviewRequired);
    pr1.repo = "org/repo-a".to_string();
    let mut pr2 = make_review_pr(2, "bob", ReviewDecision::ReviewRequired);
    pr2.repo = "org/repo-b".to_string();
    app.review_prs = vec![pr1, pr2];
    app.update(Message::SwitchToReviewBoard);

    // Toggle all on
    app.update(Message::ToggleAllReviewRepoFilter);
    assert_eq!(app.review_repo_filter.len(), 2);

    // Toggle all off
    app.update(Message::ToggleAllReviewRepoFilter);
    assert!(app.review_repo_filter.is_empty());
}

#[test]
fn review_repo_filter_toggle_single() {
    let mut app = make_app();
    let mut pr1 = make_review_pr(1, "alice", ReviewDecision::ReviewRequired);
    pr1.repo = "org/repo-a".to_string();
    app.review_prs = vec![pr1];
    app.update(Message::SwitchToReviewBoard);

    // Toggle on
    app.update(Message::ToggleReviewRepoFilter("org/repo-a".to_string()));
    assert!(app.review_repo_filter.contains("org/repo-a"));

    // Toggle off
    app.update(Message::ToggleReviewRepoFilter("org/repo-a".to_string()));
    assert!(!app.review_repo_filter.contains("org/repo-a"));
}

#[test]
fn review_repo_filter_clamps_selection() {
    let mut app = make_app();
    let mut pr1 = make_review_pr(1, "alice", ReviewDecision::ReviewRequired);
    pr1.repo = "org/repo-a".to_string();
    let mut pr2 = make_review_pr(2, "bob", ReviewDecision::ReviewRequired);
    pr2.repo = "org/repo-b".to_string();
    app.review_prs = vec![pr1, pr2];
    app.update(Message::SwitchToReviewBoard);

    // Select the second row
    if let Some(sel) = app.review_selection_mut() {
        sel.selected_row[0] = 1;
    }

    // Filter to only one PR, selection should clamp
    app.update(Message::ToggleReviewRepoFilter("org/repo-a".to_string()));
    let row = app.review_selection().unwrap().selected_row[0];
    assert_eq!(row, 0);
}

#[test]
fn review_repo_filter_selected_pr_uses_filter() {
    let mut app = make_app();
    let mut pr1 = make_review_pr(1, "alice", ReviewDecision::ReviewRequired);
    pr1.repo = "org/repo-a".to_string();
    let mut pr2 = make_review_pr(2, "bob", ReviewDecision::ReviewRequired);
    pr2.repo = "org/repo-b".to_string();
    app.review_prs = vec![pr1, pr2];
    app.update(Message::SwitchToReviewBoard);

    // Without filter, first PR is selected
    let selected = app.selected_review_pr().unwrap();
    assert_eq!(selected.number, 1);

    // Filter to repo-b only, first visible PR should be #2
    app.review_repo_filter.insert("org/repo-b".to_string());
    let selected = app.selected_review_pr().unwrap();
    assert_eq!(selected.number, 2);
}

#[test]
fn review_board_default_mode_is_reviewer() {
    let mut app = make_app();
    app.update(Message::SwitchToReviewBoard);
    match app.view_mode() {
        ViewMode::ReviewBoard { mode, .. } => {
            assert_eq!(*mode, ReviewBoardMode::Reviewer);
        }
        _ => panic!("expected ReviewBoard"),
    }
}

#[test]
fn my_prs_loaded_updates_state() {
    let mut app = make_app();
    let prs = vec![make_review_pr(101, "me", ReviewDecision::ReviewRequired)];
    app.update(Message::MyPrsLoaded(prs));
    assert_eq!(app.my_prs().len(), 1);
    assert_eq!(app.my_prs()[0].number, 101);
    assert!(!app.my_prs_loading());
}
