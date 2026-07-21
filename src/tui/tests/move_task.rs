#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Tests for the move-task-to-epic tree picker (the `m` key on a task card).

use super::*;
use crate::models::{EpicId, TaskId, TaskStatus};
use crate::tui::messages::TaskMessage;
use crossterm::event::KeyCode;

/// Build a `MoveTaskPickerState` for `task_id` with a default tree state.
fn make_move_task_picker(task_id: TaskId) -> crate::tui::MoveTaskPickerState {
    crate::tui::MoveTaskPickerState {
        task_id,
        tree_state: std::cell::RefCell::new(tui_tree_widget::TreeState::default()),
        items: vec![],
    }
}

#[test]
fn start_move_to_epic_sets_mode_and_picker() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];

    app.update(Message::Task(TaskMessage::StartMoveToEpic(TaskId(1))));

    assert_eq!(app.input.mode, InputMode::MoveTaskToEpic(TaskId(1)));
    assert!(app.interaction.move_task_picker.is_some());
    assert_eq!(
        app.interaction.move_task_picker.as_ref().unwrap().task_id,
        TaskId(1)
    );
}

#[test]
fn move_to_epic_navigate_does_not_panic() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.update(Message::Task(TaskMessage::StartMoveToEpic(TaskId(1))));

    app.update(Message::Task(TaskMessage::MoveToEpicNavigate(
        crate::tui::types::TreeNav::Down,
    )));
    app.update(Message::Task(TaskMessage::MoveToEpicNavigate(
        crate::tui::types::TreeNav::Up,
    )));
}

#[test]
fn move_to_epic_confirm_with_no_parent_selected_detaches() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.update(Message::Task(TaskMessage::StartMoveToEpic(TaskId(1))));
    // select_first picks the "— no parent —" sentinel.
    if let Some(picker) = &app.interaction.move_task_picker {
        picker.tree_state.borrow_mut().select_first();
    }

    app.update(Message::Task(TaskMessage::MoveToEpicConfirm));

    assert!(
        matches!(
            app.input.mode,
            InputMode::ConfirmMoveTaskToEpic {
                task_id: TaskId(1),
                new_epic: None
            }
        ),
        "expected ConfirmMoveTaskToEpic with no epic, got {:?}",
        app.input.mode
    );
    assert!(app.status.message.is_some());
}

#[test]
fn move_to_epic_execute_emits_command_and_resets_state() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(20)];
    app.input.mode = InputMode::ConfirmMoveTaskToEpic {
        task_id: TaskId(1),
        new_epic: Some(EpicId(20)),
    };
    app.interaction.move_task_picker = Some(make_move_task_picker(TaskId(1)));

    let cmds = app.update(Message::Task(TaskMessage::MoveToEpicExecute));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.interaction.move_task_picker.is_none());
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::MoveToEpic {
            id: TaskId(1),
            new_epic: Some(EpicId(20)),
        })
    )));
}

#[test]
fn move_to_epic_execute_with_detach_emits_none() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.input.mode = InputMode::ConfirmMoveTaskToEpic {
        task_id: TaskId(1),
        new_epic: None,
    };
    app.interaction.move_task_picker = Some(make_move_task_picker(TaskId(1)));

    let cmds = app.update(Message::Task(TaskMessage::MoveToEpicExecute));

    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::MoveToEpic {
            id: TaskId(1),
            new_epic: None,
        })
    )));
}

#[test]
fn move_to_epic_cancel_from_confirm_returns_to_picker() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.input.mode = InputMode::ConfirmMoveTaskToEpic {
        task_id: TaskId(1),
        new_epic: None,
    };
    app.interaction.move_task_picker = Some(make_move_task_picker(TaskId(1)));

    app.update(Message::Task(TaskMessage::MoveToEpicCancel));

    assert_eq!(app.input.mode, InputMode::MoveTaskToEpic(TaskId(1)));
    assert!(app.interaction.move_task_picker.is_some());
}

#[test]
fn move_to_epic_cancel_from_picker_clears_state() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.input.mode = InputMode::MoveTaskToEpic(TaskId(1));
    app.interaction.move_task_picker = Some(make_move_task_picker(TaskId(1)));

    app.update(Message::Task(TaskMessage::MoveToEpicCancel));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.interaction.move_task_picker.is_none());
}

#[test]
fn move_to_epic_cancel_all_from_confirm_clears_state() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.input.mode = InputMode::ConfirmMoveTaskToEpic {
        task_id: TaskId(1),
        new_epic: None,
    };
    app.interaction.move_task_picker = Some(make_move_task_picker(TaskId(1)));

    app.update(Message::Task(TaskMessage::MoveToEpicCancelAll));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.interaction.move_task_picker.is_none());
}

#[test]
fn m_key_on_task_card_opens_move_picker() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1); // Backlog column
    app.selection_mut().set_row(1, 0); // first row — the standalone task

    // Sanity: cursor is on the task, not an epic.
    assert!(app.selected_task().is_some());

    app.handle_key(make_key(KeyCode::Char('m')));

    assert_eq!(app.input.mode, InputMode::MoveTaskToEpic(TaskId(1)));
    assert!(app.interaction.move_task_picker.is_some());
}
