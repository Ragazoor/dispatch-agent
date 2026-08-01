#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{TaskId, TaskStatus};
use crossterm::event::KeyCode;

#[test]
fn confirm_done_y_moves_task() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)]);
    app.selection_mut().set_column(3);

    app.prompt_move_to_done(vec![TaskId(1)]);
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[test]
fn confirm_done_n_cancels() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)]);
    app.selection_mut().set_column(3);

    app.prompt_move_to_done(vec![TaskId(1)]);
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert!(cmds.is_empty());
}

#[test]
fn confirm_done_kills_tmux_but_preserves_worktree() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-test".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }]);
    app.selection_mut().set_column(3);

    // Enter confirm mode and confirm
    app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(1),
        direction: MoveDirection::Forward,
    }));
    assert_eq!(app.input.mode, InputMode::ConfirmDone);

    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::ConfirmDone,
    ));
    // No Cleanup command — worktree stays for archive to clean up later
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
    )));
    // Tmux window should be killed
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::KillTmuxWindow { .. })
    )));
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
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
    ]);
    app.selection_mut().set_column(3);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('L'))));
    assert!(cmds.is_empty());
    assert!(app.status.message.as_deref().unwrap().contains("2 tasks"));
    assert!(app.status.message.as_deref().unwrap().contains("Done"));
}

#[test]
fn batch_confirm_done_moves_all_review_tasks() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
        make_task(2, TaskStatus::Review),
    ]);
    app.selection_mut().set_column(3);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    // Trigger batch move
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::BatchMove {
            ids: vec![TaskId(1), TaskId(2)],
            direction: MoveDirection::Forward,
        },
    ));
    // Confirm
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::ConfirmDone,
    ));
    assert_eq!(app.input.mode, InputMode::Normal);
    for id in [TaskId(1), TaskId(2)] {
        let task = app.board.tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(task.status, TaskStatus::Done);
    }
    assert!(cmds.len() >= 2); // two PersistTask commands
}

#[test]
fn handle_key_confirm_done_yes() {
    let mut app = make_app();
    // Move task 3 (Running) to Review so ConfirmDone makes sense
    let task_3 = app
        .board
        .tasks
        .iter_mut()
        .find(|t| t.id == TaskId(3))
        .unwrap();
    task_3.status = TaskStatus::Review;
    app.prompt_move_to_done(vec![TaskId(3)]);

    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(*app.mode(), InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Persist(t)) if t.id == TaskId(3) && t.status == TaskStatus::Done)));
}

#[test]
fn handle_key_confirm_done_cancel() {
    let mut app = make_app();
    app.prompt_move_to_done(vec![TaskId(3)]);
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn render_status_bar_confirm_done() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDone;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Done?"),
        "ConfirmDone should show 'Done?'"
    );
}

/// ConfirmDone mode routes correctly.
#[test]
fn handle_key_confirm_done_routes_correctly() {
    let mut app = make_app();
    app.prompt_move_to_done(vec![TaskId(1)]);
    // 'n' cancels
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// P key opens the TODO overlay (emits a Todo(Load) command).
#[test]
fn p_uppercase_key_opens_todos() {
    use crate::tui::commands::TodoCommand;
    use crate::tui::types::Command;
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('P')));
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::Todo(TodoCommand::Load))),
        "P key should emit a Todo(Load) command, got: {cmds:?}"
    );
}

// ---------------------------------------------------------------------------
// `W` removal (docs/plans/3813-keybinding-pruning-implementation.md §1):
// the board's wrap-up entry point is gone. Wrap-up is agent-driven only,
// through the MCP `wrap_up` tool — never through a TUI key.
// ---------------------------------------------------------------------------

#[test]
fn w_key_is_inert_on_review_task_with_worktree() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }]);
    let mode_before = app.input.mode.clone();

    let cmds = app.handle_key(make_key(KeyCode::Char('W')));

    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, mode_before);
}

#[test]
fn w_key_is_inert_on_epic() {
    let mut app = App::new(vec![make_review_subtask(1, 10, 1)]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Review;
    app.board.epics = vec![epic];
    app.selection_mut().set_column(3);
    app.selection_mut().set_row(3, 0);
    let mode_before = app.input.mode.clone();

    let cmds = app.handle_key(make_key(KeyCode::Char('W')));

    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, mode_before);
}

#[test]
fn status_bar_no_longer_shows_wrap_up_hint_for_review_task() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    let mut app = App::new(vec![task]);
    // Navigate to Review column (index 2)
    for _ in 0..2 {
        app.update(Message::NavigateColumn(1));
    }

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        !buffer_contains(&buf, "[W]rap up"),
        "Status bar should no longer show a wrap up hint for Review tasks"
    );
}

#[test]
fn epic_action_hints_no_longer_shows_wrap_up_hint() {
    let epic = make_epic(10);
    let hints = crate::tui::ui::epic_action_hints(&epic, ratatui::style::Color::White);
    let joined: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        !joined.contains("[W]"),
        "epic_action_hints should no longer advertise the W key, got: {joined:?}"
    );
}
