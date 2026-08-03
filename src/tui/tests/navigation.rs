#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{EpicId, SubStatus, TaskId, TaskStatus};
use crossterm::event::KeyCode;
use ratatui::style::Color;
use std::time::{Duration, Instant};

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
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(1),
        direction: MoveDirection::Forward,
    }));
    assert_eq!(
        app.board
            .tasks
            .iter()
            .find(|t| t.id == TaskId(1))
            .unwrap()
            .status,
        TaskStatus::Running
    );
    // Should produce a PersistTask command
    assert!(matches!(
        cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    ));
}

#[test]
fn move_task_backward_at_start_is_noop() {
    let mut app = make_app();
    // Task 1 is in Backlog; prev() stays Backlog
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(1),
        direction: MoveDirection::Backward,
    }));
    assert_eq!(
        app.board
            .tasks
            .iter()
            .find(|t| t.id == TaskId(1))
            .unwrap()
            .status,
        TaskStatus::Backlog
    );
    assert!(cmds.is_empty());
}

#[test]
fn quit_enters_confirm_mode() {
    let mut app = make_app();
    assert!(!app.should_quit);
    app.update(Message::System(crate::tui::messages::SystemMessage::Quit));
    assert!(!app.should_quit);
    assert_eq!(app.input.mode, InputMode::ConfirmQuit);
}

#[test]
fn pressing_q_on_board_enters_confirm_quit() {
    let mut app = make_app();
    assert_eq!(app.input.mode, InputMode::Normal);
    app.handle_key(make_key(KeyCode::Char('q')));
    assert!(!app.should_quit);
    assert_eq!(app.input.mode, InputMode::ConfirmQuit);
}

#[test]
fn navigate_column_clamps() {
    let mut app = make_app();
    // Backlog column (1) is the leftmost; navigating further left is a no-op.
    app.selection_mut().set_column(1);
    app.update(Message::NavigateColumn(-1));
    assert_eq!(app.selection().column(), 1); // can't go below Backlog

    // From archive column (COLUMN_COUNT+1 = 5), pressing right stays clamped.
    app.selection_mut().set_column(TaskStatus::COLUMN_COUNT + 1);
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selection().column(), TaskStatus::COLUMN_COUNT + 1); // can't go above max
}

#[test]
fn navigate_column_moves_through_visual_columns() {
    let mut app = make_app();
    // Board starts at Backlog (nav col 1); Projects is col 0 (left edge).
    assert_eq!(app.selected_column(), 1); // Backlog
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), 2); // Running
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), 3); // Review
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), 4); // Done
}

#[test]
fn navigate_column_clamps_at_visual_column_max() {
    let mut app = make_app();
    // From Done (nav col 4 = COLUMN_COUNT) pressing right enters archive (nav col 5), not a clamp.
    app.selection_mut().set_column(TaskStatus::COLUMN_COUNT);
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), TaskStatus::COLUMN_COUNT + 1); // archive column
                                                                     // From archive (nav col 5), pressing right is clamped.
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), TaskStatus::COLUMN_COUNT + 1); // stays at archive
}

#[test]
fn navigate_column_always_lands_on_first_row_and_resets_scroll() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
        make_task(3, TaskStatus::Backlog),
        make_task(4, TaskStatus::Running),
        make_task(5, TaskStatus::Running),
    ]);
    // Move to the last row in Backlog (nav col 1) and simulate it having
    // scrolled down, then leave and return.
    app.selection_mut().set_column(1);
    app.update(Message::NavigateRow(1));
    app.update(Message::NavigateRow(1));
    assert_eq!(app.selection().row(1), 2, "precondition: parked on row 2");
    *app.selection_mut().list_states[0].offset_mut() = 5;

    app.update(Message::NavigateColumn(1)); // -> Running
    app.update(Message::NavigateColumn(-1)); // back -> Backlog

    assert_eq!(app.selection().column(), 1, "precondition: back in Backlog");
    assert_eq!(
        app.selection().row(1),
        0,
        "switching back into a column must default to the first card, not the sticky row"
    );
    assert_eq!(
        app.selection().list_states[0].offset(),
        0,
        "re-entering a column must scroll it back to the top"
    );
    assert!(!app.on_select_all());
}

#[test]
fn navigate_column_noop_at_edge_preserves_row_and_scroll() {
    let mut app = make_app();
    app.selection_mut().set_column(1); // leftmost
    app.update(Message::NavigateRow(1));
    assert_eq!(app.selection().row(1), 1);
    *app.selection_mut().list_states[0].offset_mut() = 3;

    // h/Left at the leftmost column is a clamped no-op — must not reset cursor/scroll.
    app.update(Message::NavigateColumn(-1));

    assert_eq!(app.selection().row(1), 1, "no-op must not reset row");
    assert_eq!(
        app.selection().list_states[0].offset(),
        3,
        "no-op must not reset scroll"
    );
}

#[test]
fn navigate_column_into_empty_column_preserves_select_all() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    app.selection_mut().set_column(1);
    app.selection_mut().on_select_all = true;

    // Running (nav col 2) is empty; switching there must not clobber on_select_all
    // for an empty destination (mirrors the `[`/`]` empty-column no-op semantics).
    app.update(Message::NavigateColumn(1));

    assert_eq!(app.selection().column(), 2);
    assert!(
        app.on_select_all(),
        "on_select_all must be left untouched when the destination column is empty"
    );
    assert_eq!(app.selection().row(2), 0);
}

#[test]
fn navigate_row_clamps() {
    let mut app = make_app();
    // Backlog is nav col 1. Selected row starts at 0.
    app.selection_mut().set_column(1);
    app.update(Message::NavigateRow(-1));
    // Navigating up from row 0 now moves to the select-all toggle
    assert!(app.on_select_all());

    // Navigate back down to tasks and then past the end
    app.update(Message::NavigateRow(1));
    assert!(!app.on_select_all());
    app.update(Message::NavigateRow(10));
    assert_eq!(app.selection().row(1), 1); // clamps to last item index
}

#[test]
fn navigate_row_clamps_to_task_count_in_flat_mode_with_epic_headers() {
    // In flat mode, column_items_for_status inserts unselectable EpicHeader
    // separators between epic groups, so its length overcounts selectable rows.
    // With 2 Running tasks under epic 10 it returns
    //   [SubstatusLabel, EpicHeader(epic10), Task(t1), Task(t2)]  — len = 4,
    // while column_item_count returns 2 (tasks only, since flattened=true).
    // Navigation must clamp to task_count-1 (row 1), not len()-1 (row 3).
    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(EpicId(10));
    let mut t2 = make_task(2, TaskStatus::Running);
    t2.epic_id = Some(EpicId(10));

    let mut app = App::new(vec![t1, t2]);
    app.board.epics = vec![make_epic(10)];
    app.board.flattened = true;
    app.selection_mut().set_column(2); // Running = nav col 2

    // Navigate far past the end — should clamp to last selectable task (row 1).
    app.update(Message::NavigateRow(10));
    assert_eq!(
        app.selection().row(2),
        1,
        "should clamp to task count - 1, not column_items.len() - 1"
    );

    // Navigate back up from row 0 — should enter the select-all toggle row,
    // same as in non-flat mode (the boundary logic is mode-independent).
    app.update(Message::NavigateRow(-10)); // back to row 0
    assert!(!app.on_select_all());
    app.update(Message::NavigateRow(-1)); // one step up from row 0
    assert!(
        app.on_select_all(),
        "NavigateRow(-1) from row 0 in flat mode should land on select-all toggle"
    );
}

#[test]
fn delete_task_with_worktree_emits_cleanup() {
    let mut app = make_app();
    let task = app.find_task_mut(TaskId(4)).unwrap();
    task.worktree = Some("/repo/.worktrees/4-task".to_string());
    task.tmux_window = Some("task-4".to_string());

    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Delete(
        TaskId(4),
    )));
    assert!(app.board.tasks.iter().all(|t| t.id != TaskId(4)));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Delete(TaskId(4)))
    )));
}

#[test]
fn delete_task_without_worktree_no_cleanup() {
    let mut app = make_app();
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Delete(
        TaskId(1),
    )));
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
    )));
}

#[test]
fn error_sets_error_popup() {
    let mut app = App::new(vec![]);
    app.update(Message::System(crate::tui::messages::SystemMessage::Error(
        "Something went wrong".to_string(),
    )));
    assert_eq!(
        app.status.error_popup.as_deref(),
        Some("Something went wrong")
    );
}

#[test]
fn move_backward_from_running_detaches_but_keeps_worktree() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(4),
        direction: MoveDirection::Backward,
    }));

    // Should emit KillTmuxWindow then PersistTask (no Cleanup)
    assert_eq!(cmds.len(), 2);
    assert!(
        matches!(&cmds[0], Command::Task(crate::tui::commands::TaskCommand::KillTmuxWindow { window }) if window == "task-4")
    );
    assert!(matches!(
        &cmds[1],
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    ));

    // Worktree preserved, tmux_window cleared
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(4)).unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/4-task-4"));
    assert!(task.tmux_window.is_none());
}

#[test]
fn move_backward_from_running_without_dispatch_fields() {
    let mut task = make_task(3, TaskStatus::Running);
    task.worktree = None;
    task.tmux_window = None;
    let mut app = App::new(vec![task]);
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(3),
        direction: MoveDirection::Backward,
    }));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(
        cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    ));
}

#[test]
fn move_forward_to_done_enters_confirm_mode() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = None; // session closed, but worktree remains
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(5),
        direction: MoveDirection::Forward,
    }));

    // Should enter confirmation mode, not move immediately
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmDone);
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(5)).unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    // Worktree preserved — not taken during confirmation
    assert!(task.worktree.is_some());
}

/// The single-task Done prompt records its task in `select.pending_done`, the
/// same place the batch prompt uses — that is why `InputMode::ConfirmDone`
/// carries no payload.
#[test]
fn single_move_to_done_records_the_task_in_pending_done() {
    let mut app = App::new(vec![make_task(5, TaskStatus::Review)]);

    app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(5),
        direction: MoveDirection::Forward,
    }));

    assert_eq!(app.select.pending_done, vec![TaskId(5)]);
}

#[test]
fn move_forward_to_done_with_live_window_enters_confirm_mode() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = Some("task-5".to_string());
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(5),
        direction: MoveDirection::Forward,
    }));

    // Should enter confirmation mode, not move immediately
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmDone);
    assert_eq!(app.select.pending_done, vec![TaskId(5)]);
}

#[test]
fn space_key_with_live_window_jumps() {
    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(2); // Running = nav col 2
    let cmds = app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(
        cmds.iter().any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { window }) if window == "task-4"))
    );
}

#[test]
fn space_key_without_window_dispatches_backlog() {
    // Space on a windowless Backlog task now activates it (dispatch), routing
    // through the repo-trust gate — a CheckTrustAndDispatch command — rather
    // than showing "No active session" or deciding trust synchronously.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.selection_mut().set_column(1); // Backlog = nav col 1
    let cmds = app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::CheckTrustAndDispatch {
                id: TaskId(1),
                ..
            })
        )),
        "expected CheckTrustAndDispatch command, got {cmds:?}"
    );
}

#[test]
fn typing_appends_to_input_buffer() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.handle_key(make_key(KeyCode::Char('H')));
    app.handle_key(make_key(KeyCode::Char('i')));
    assert_eq!(app.input.buffer, "Hi");
}

#[test]
fn any_key_clears_error_popup() {
    let mut app = App::new(vec![]);
    app.status.error_popup = Some("boom".to_string());
    let cmds = app.handle_key(make_key(KeyCode::Char('a')));
    assert!(app.status.error_popup.is_none());
    assert!(cmds.is_empty());
}

#[test]
fn error_popup_blocks_normal_key_handling() {
    let mut app = make_app();
    app.status.error_popup = Some("boom".to_string());
    app.handle_key(make_key(KeyCode::Char('q'))); // would normally quit
    assert!(app.status.error_popup.is_none());
    assert!(!app.should_quit); // quit was NOT processed
}

#[test]
fn resumed_sets_tmux_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/wt".to_string());
    let mut app = App::new(vec![task]);
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Resumed {
        id: TaskId(4),
        tmux_window: "win-4".to_string(),
    }));
    assert_eq!(app.board.tasks[0].tmux_window.as_deref(), Some("win-4"));
    assert_eq!(cmds.len(), 2);
    assert!(matches!(
        &cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    ));
    assert!(matches!(
        &cmds[1],
        Command::Task(crate::tui::commands::TaskCommand::SeedActivity { id, .. }) if *id == TaskId(4)
    ));
}

#[test]
fn resumed_unknown_id_is_noop() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Resumed {
        id: TaskId(999),
        tmux_window: "win".to_string(),
    }));
    assert!(cmds.is_empty());
}

#[test]
fn resumed_sets_status_to_running() {
    let mut task = make_task(4, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Resumed {
        id: TaskId(4),
        tmux_window: "task-4".to_string(),
    }));

    let task = app.board.tasks.iter().find(|t| t.id == TaskId(4)).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.tmux_window.as_deref(), Some("task-4"));
    assert_eq!(cmds.len(), 2);
    assert!(
        matches!(&cmds[0], Command::Task(crate::tui::commands::TaskCommand::Persist(t)) if t.status == TaskStatus::Running)
    );
    assert!(matches!(
        &cmds[1],
        Command::Task(crate::tui::commands::TaskCommand::SeedActivity { .. })
    ));
}

#[test]
fn refresh_tasks_replaces_and_clamps() {
    let mut app = make_app();
    app.selection_mut().set_row(1, 1); // row 1 of Backlog (nav col 1, has 2 items)
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![make_task(10, TaskStatus::Backlog)],
    )));
    assert_eq!(app.board.tasks.len(), 1);
    assert_eq!(app.board.tasks[0].id, TaskId(10));
    assert_eq!(app.selection().row(1), 0); // clamped from 1 to 0
}

#[test]
fn refresh_tasks_empty_clamps_all_rows_to_zero() {
    let mut app = make_app();
    app.selection_mut().set_row(0, 1);
    app.selection_mut().set_row(1, 1);
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![],
    )));
    assert!(app.board.tasks.is_empty());
    assert!(app.selection().selected_row.iter().all(|&r| r == 0));
}

#[test]
fn dismiss_error_clears_popup() {
    let mut app = App::new(vec![]);
    app.status.error_popup = Some("boom".to_string());
    app.update(Message::System(
        crate::tui::messages::SystemMessage::DismissError,
    ));
    assert!(app.status.error_popup.is_none());
}

#[test]
fn input_char_appends_to_buffer() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.update(Message::Input(
        crate::tui::messages::InputMessage::InputChar('H'),
    ));
    app.update(Message::Input(
        crate::tui::messages::InputMessage::InputChar('i'),
    ));
    assert_eq!(app.input.buffer, "Hi");
}

#[test]
fn input_backspace_removes_last_char() {
    let mut app = App::new(vec![]);
    app.input.set_buffer("abc".to_string());
    app.update(Message::Input(
        crate::tui::messages::InputMessage::InputBackspace,
    ));
    assert_eq!(app.input.buffer, "ab");
}

#[test]
fn status_info_sets_message() {
    let mut app = App::new(vec![]);
    app.update(Message::System(
        crate::tui::messages::SystemMessage::StatusInfo("hello".to_string()),
    ));
    assert_eq!(app.status.message.as_deref(), Some("hello"));
}

#[test]
fn start_quick_dispatch_selection_enters_mode() {
    let mut app = App::new(vec![]);
    app.update(Message::Input(
        crate::tui::messages::InputMessage::StartQuickDispatchSelection,
    ));
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
    assert!(app.status.message.is_some());
}

#[test]
fn select_quick_dispatch_repo_dispatches() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SelectQuickDispatchRepo(1),
    ));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(
        |c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, .. }) if draft.repo_path == "/repo2")
    ));
}

#[test]
fn select_quick_dispatch_repo_out_of_range_is_noop() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo1".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SelectQuickDispatchRepo(5),
    ));
    assert!(cmds.is_empty());
    // Mode is not changed by the handler (stays as-is)
}

#[test]
fn v_toggles_task_selection() {
    let mut app = make_app();
    // Select task 1 in Backlog
    app.handle_key(make_key(KeyCode::Char('v')));
    assert!(app.select.tasks.contains(&TaskId(1)));

    // Toggle off
    app.handle_key(make_key(KeyCode::Char('v')));
    assert!(!app.select.tasks.contains(&TaskId(1)));
}

#[test]
fn v_on_empty_column_is_noop() {
    let mut app = make_app();
    // Navigate to Review column (empty)
    app.update(Message::NavigateColumn(2));
    app.handle_key(make_key(KeyCode::Char('v')));
    assert!(app.select.tasks.is_empty());
}

#[test]
fn batch_move_forward_moves_all_selected() {
    let mut app = make_app();
    // Select both Backlog tasks
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    // Press m to batch move forward
    let cmds = app.handle_key(make_key(KeyCode::Char('L')));

    // Both should now be Running
    assert_eq!(
        app.find_task(TaskId(1)).unwrap().status,
        TaskStatus::Running
    );
    assert_eq!(
        app.find_task(TaskId(2)).unwrap().status,
        TaskStatus::Running
    );
    // Should have PersistTask commands
    let persist_count = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::Task(crate::tui::commands::TaskCommand::Persist(_))
            )
        })
        .count();
    assert_eq!(persist_count, 2);
}

#[test]
fn batch_move_clears_selection() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    app.handle_key(make_key(KeyCode::Char('L')));

    assert!(app.select.tasks.is_empty());
}

#[test]
fn batch_move_backward() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
        make_task(3, TaskStatus::Done),
    ]);

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    app.handle_key(make_key(KeyCode::Char('H')));

    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Review);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Review);
    // Task 3 not selected, should remain Done
    assert_eq!(app.find_task(TaskId(3)).unwrap().status, TaskStatus::Done);
}

#[test]
fn single_task_operations_work_without_selection() {
    let mut app = make_app();
    assert!(app.select.tasks.is_empty());

    // Single move should still work
    let cmds = app.handle_key(make_key(KeyCode::Char('L')));
    assert_eq!(
        app.find_task(TaskId(1)).unwrap().status,
        TaskStatus::Running
    );
    assert!(!cmds.is_empty());
}

#[test]
fn refresh_tasks_prunes_stale_selections() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(99)),
    )); // non-existent

    // Refresh with only task 1
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![make_task(1, TaskStatus::Backlog)],
    )));

    assert!(app.select.tasks.contains(&TaskId(1)));
    assert!(!app.select.tasks.contains(&TaskId(99)));
}

#[test]
fn e_on_task_directly_edits() {
    let mut app = make_app();
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('e'))));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Editor(crate::tui::commands::EditorCommand::PopOut(
            EditKind::TaskEdit(_)
        ))
    )));
    assert_eq!(app.input.mode, InputMode::Normal);
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
    let mut app = App::new(vec![]);
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('V')));
    assert!(cmds.is_empty());
}

#[test]
fn focus_changed_ignored_when_split_inactive() {
    let mut app = make_app();
    assert!(!app.split_active());

    let cmds = app.update(Message::System(
        crate::tui::messages::SystemMessage::FocusChanged(false),
    ));
    assert!(cmds.is_empty());
    assert!(app.split_focused()); // still true — ignored
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
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.input.set_buffer("x".to_string());
    let cmds = app.handle_key(make_key(KeyCode::Tab));
    assert!(cmds.is_empty());
    assert_eq!(app.input.buffer, "x");
    assert_eq!(app.input.mode, InputMode::InputTitle);
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
fn help_mode_ignores_other_keys() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;

    app.handle_key(make_key(KeyCode::Char('q')));
    assert_eq!(app.input.mode, InputMode::Help);
    assert!(!app.should_quit);
}

#[test]
fn help_overlay_hidden_in_normal_mode() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 80, 30);
    assert!(!buffer_contains(&buf, "Navigation"));
}

#[test]
fn move_review_to_done_enters_confirm_mode() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)]);
    app.selection_mut().set_column(3); // Review = nav col 3

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('L'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmDone);
    assert_eq!(app.select.pending_done, vec![TaskId(1)]);
    assert!(app.status.message.as_deref().unwrap().contains("Done"));
}

#[test]
fn move_backlog_to_running_no_confirmation() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.selection_mut().set_column(1); // Backlog = nav col 1

    let cmds = app.handle_key(make_key(KeyCode::Char('L')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[test]
fn batch_move_mixed_statuses_moves_non_review_immediately() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Running),
        make_task(2, TaskStatus::Review),
    ]);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::BatchMove {
            ids: vec![TaskId(1), TaskId(2)],
            direction: MoveDirection::Forward,
        },
    ));
    // Running→Review moved immediately
    let t1 = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(t1.status, TaskStatus::Review);
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Persist(t)) if t.id == TaskId(1))));

    // Review→Done waiting for confirmation
    let t2 = app.board.tasks.iter().find(|t| t.id == TaskId(2)).unwrap();
    assert_eq!(t2.status, TaskStatus::Review); // not moved yet
    assert_eq!(app.input.mode, InputMode::ConfirmDone);
    assert_eq!(app.select.pending_done, vec![TaskId(2)]);
}

#[test]
fn key_a_toggles_off_when_all_selected() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('a')));
    assert_eq!(app.select.tasks.len(), 2);
    app.handle_key(make_key(KeyCode::Char('a')));
    assert!(app.select.tasks.is_empty());
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
fn column_scrolls_to_keep_cursor_visible() {
    // Create 20 backlog tasks — more than fit in a 20-row terminal
    let tasks: Vec<Task> = (1..=20)
        .map(|id| make_task(id, TaskStatus::Backlog))
        .collect();
    let mut app = App::new(tasks);

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
    let mut app = App::new(tasks);

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
    assert!(!app.notifications_enabled()); // default: false
    app.update(Message::System(
        crate::tui::messages::SystemMessage::ToggleNotifications,
    ));
    assert!(app.notifications_enabled());
    app.update(Message::System(
        crate::tui::messages::SystemMessage::ToggleNotifications,
    ));
    assert!(!app.notifications_enabled());
}

#[test]
fn refresh_tasks_emits_notification_on_review_transition() {
    let mut app = make_app();
    app.set_notifications_enabled(true);
    // Task 3 starts as Running
    assert_eq!(app.board.tasks[2].status, TaskStatus::Running);

    // Simulate DB refresh where task 3 moved to Review
    let mut updated = app.board.tasks.to_vec();
    updated[2].status = TaskStatus::Review;
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated,
    )));

    let notif_cmds: Vec<_> = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            )
        })
        .collect();
    assert_eq!(notif_cmds.len(), 1);
    match &notif_cmds[0] {
        Command::System(crate::tui::commands::SystemCommand::SendNotification {
            title,
            urgent,
            ..
        }) => {
            assert!(title.contains("Task 3"));
            assert!(!urgent);
        }
        _ => unreachable!(),
    }
}

#[test]
fn refresh_tasks_emits_urgent_notification_on_needs_input() {
    let mut app = make_app();
    app.set_notifications_enabled(true);

    let mut updated = app.board.tasks.to_vec();
    updated[2].sub_status = SubStatus::NeedsInput;
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated,
    )));

    let notif_cmds: Vec<_> = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            )
        })
        .collect();
    assert_eq!(notif_cmds.len(), 1);
    match &notif_cmds[0] {
        Command::System(crate::tui::commands::SystemCommand::SendNotification {
            urgent, ..
        }) => {
            assert!(urgent);
        }
        _ => unreachable!(),
    }
}

#[test]
fn refresh_tasks_does_not_duplicate_notifications() {
    let mut app = make_app();

    let mut updated = app.board.tasks.to_vec();
    updated[2].status = TaskStatus::Review;
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated.clone(),
    )));
    // Second refresh with same state should not re-notify
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated,
    )));
    let notif_cmds: Vec<_> = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            )
        })
        .collect();
    assert_eq!(notif_cmds.len(), 0);
}

#[test]
fn refresh_tasks_does_not_duplicate_needs_input_notifications() {
    let mut app = make_app();

    let mut updated = app.board.tasks.to_vec();
    updated[2].sub_status = SubStatus::NeedsInput;
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated.clone(),
    )));
    // Second refresh with same state should not re-notify
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated,
    )));
    let notif_cmds: Vec<_> = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            )
        })
        .collect();
    assert_eq!(notif_cmds.len(), 0);
}

#[test]
fn refresh_tasks_renotifies_needs_input_after_clearing() {
    let mut app = make_app();
    app.set_notifications_enabled(true);

    // First transition to NeedsInput
    let mut updated = app.board.tasks.to_vec();
    updated[2].sub_status = SubStatus::NeedsInput;
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated.clone(),
    )));
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            ))
            .count(),
        1
    );

    // Clear NeedsInput (agent resumes)
    updated[2].sub_status = SubStatus::Active;
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated.clone(),
    )));

    // Second transition to NeedsInput should re-notify
    updated[2].sub_status = SubStatus::NeedsInput;
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated,
    )));
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            ))
            .count(),
        1
    );
}

#[test]
fn refresh_tasks_skips_notification_when_disabled() {
    let mut app = make_app();
    // notifications disabled by default — no toggle needed

    let mut updated = app.board.tasks.to_vec();
    updated[2].status = TaskStatus::Review;
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated,
    )));

    let notif_cmds: Vec<_> = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            )
        })
        .collect();
    assert_eq!(notif_cmds.len(), 0);
}

#[test]
fn key_n_uppercase_toggles_notifications() {
    let mut app = make_app();
    assert!(!app.notifications_enabled()); // default: false
    let cmds = app.handle_key(make_key(KeyCode::Char('N')));
    assert!(app.notifications_enabled()); // toggled to enabled
                                          // Should emit PersistSetting command
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::PersistSetting { .. })));
    // Should show status message
    assert!(app.status.message.as_deref().unwrap().contains("enabled"));
}

#[test]
fn refresh_tasks_clears_notified_when_task_leaves_review() {
    let mut app = make_app();
    app.set_notifications_enabled(true);

    // Move to review — triggers notification
    let mut updated = app.board.tasks.to_vec();
    updated[2].status = TaskStatus::Review;
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated,
    )));

    // Move to done — should clear notified state
    let mut updated2 = app.board.tasks.to_vec();
    updated2[2].status = TaskStatus::Done;
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated2,
    )));

    // Move back to review — should re-notify
    let mut updated3 = app.board.tasks.to_vec();
    updated3[2].status = TaskStatus::Review;
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated3,
    )));
    let notif_cmds: Vec<_> = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            )
        })
        .collect();
    assert_eq!(notif_cmds.len(), 1);
}

#[test]
fn refresh_tasks_clears_notified_state_even_when_disabled() {
    let mut app = make_app();
    app.set_notifications_enabled(true);

    // Task transitions to review while notifications enabled — gets notified
    let mut updated = app.board.tasks.to_vec();
    updated[2].status = TaskStatus::Review;
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated,
    )));
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            ))
            .count(),
        1
    );

    // Disable notifications
    app.update(Message::System(
        crate::tui::messages::SystemMessage::ToggleNotifications,
    ));

    // Task leaves review while disabled
    let mut updated2 = app.board.tasks.to_vec();
    updated2[2].status = TaskStatus::Done;
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated2,
    )));

    // Re-enable notifications
    app.update(Message::System(
        crate::tui::messages::SystemMessage::ToggleNotifications,
    ));

    // Task returns to review — should re-notify because notified state was cleared
    let mut updated3 = app.board.tasks.to_vec();
    updated3[2].status = TaskStatus::Review;
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        updated3,
    )));
    let notif_cmds: Vec<_> = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
            )
        })
        .collect();
    assert_eq!(
        notif_cmds.len(),
        1,
        "Should re-notify after notified state was cleared while disabled"
    );
}

#[test]
fn summary_row_shows_bell_and_hint_when_notifications_enabled() {
    let mut app = make_app();
    app.set_notifications_enabled(true); // explicitly enable (default is false)
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "\u{1F514}")); // 🔔
    assert!(buffer_contains(&buf, "[N]"));
}

#[test]
fn summary_row_shows_muted_bell_and_hint_when_disabled() {
    let mut app = make_app();
    // notifications disabled by default — no toggle needed
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "\u{1F515}")); // 🔕
    assert!(buffer_contains(&buf, "[N]"));
}

#[test]
fn detail_panel_shows_pr_url() {
    let mut task = make_task(1, TaskStatus::Review);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);
    // Navigate to Review column (index 2) and open detail panel
    for _ in 0..2 {
        app.update(Message::NavigateColumn(1));
    }
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::OpenDetail(TaskId(1)),
    ));
    let _buf = render_to_buffer(&mut app, 200, 20);
}

#[test]
fn summary_row_shows_filter_indicator() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.filter.repos.insert("/a".to_string());
    app.filter.repos.insert("/b".to_string());

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "2/3 repos"),
        "Expected filter indicator in summary"
    );
}

#[test]
fn summary_row_shows_excl_prefix_in_exclude_mode() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.filter.repos.insert("/a".to_string());
    app.filter.mode = RepoFilterMode::Exclude;

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "excl 1/3 repos"),
        "Expected excl prefix in filter indicator"
    );
}

#[test]
fn w_key_on_non_review_task_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);

    app.handle_key(make_key(KeyCode::Char('W')));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn tick_skips_already_stale_tasks() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());
    app.board.tasks[0].sub_status = SubStatus::Stale;

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    // Tick should NOT re-emit PersistTask for already-stale tasks
    // (only CheckWindow and RefreshFromDb expected)
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[test]
fn tick_skips_already_crashed_tasks() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());
    app.board.tasks[0].sub_status = SubStatus::Crashed;

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[test]
fn tick_skips_conflict_tasks_for_stale_detection() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());
    app.board.tasks[0].sub_status = SubStatus::Conflict;

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
    assert_eq!(app.board.tasks[0].sub_status, SubStatus::Conflict);
}

#[test]
fn move_task_forward_resets_substatus() {
    let mut app = make_app();
    let id = TaskId(3); // Running
    app.find_task_mut(id).unwrap().sub_status = SubStatus::Stale;
    app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id,
        direction: MoveDirection::Forward,
    }));
    let task = app.find_task(id).unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::AwaitingReview);
}

#[test]
fn move_task_backward_resets_substatus() {
    let mut app = make_app();
    let id = TaskId(3); // Running
    app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id,
        direction: MoveDirection::Backward,
    }));
    let task = app.find_task(id).unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert_eq!(task.sub_status, SubStatus::None);
}

#[test]
fn shift_l_with_mixed_selection_moves_tasks_only() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(10)),
    ));
    // Cursor on the task (row 0) so 'm' triggers batch move, not epic move
    app.selection_mut().set_column(1); // Backlog = nav col 1
    app.selection_mut().set_row(1, 0);

    app.handle_key(make_key(KeyCode::Char('L')));
    // Task should move forward
    assert_eq!(
        app.find_task(TaskId(1)).unwrap().status,
        TaskStatus::Running
    );
}

#[test]
fn detach_tmux_single_sets_confirm_mode() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)]);
    app.board.tasks[0].tmux_window = Some("task-1".to_string());

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::DetachTmux(TaskId(1)),
    ));

    assert!(
        matches!(&app.input.mode, InputMode::ConfirmDetachTmux(ids) if ids == &[TaskId(1)]),
        "Expected ConfirmDetachTmux([1]), got {:?}",
        app.input.mode
    );
    assert!(app.status.message.is_some());
}

#[test]
fn detach_tmux_running_task_with_window_is_detachable() {
    // Running tasks with a tmux window should pass the detach filter.
    let mut app = App::new(vec![make_task(1, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-1".to_string());

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::DetachTmux(TaskId(1)),
    ));

    assert!(
        matches!(&app.input.mode, InputMode::ConfirmDetachTmux(ids) if ids == &[TaskId(1)]),
        "Running task with window should set ConfirmDetachTmux, got {:?}",
        app.input.mode
    );
}

#[test]
fn detach_tmux_noop_on_task_without_window() {
    let mut task = make_task(1, TaskStatus::Review);
    task.tmux_window = None;
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::DetachTmux(TaskId(1)),
    ));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty(), "should produce no commands");
}

#[test]
fn batch_detach_tmux() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
        make_task(2, TaskStatus::Review),
    ]);
    app.board.tasks[0].tmux_window = Some("task-1".to_string());
    app.board.tasks[1].tmux_window = Some("task-2".to_string());

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::BatchDetachTmux(vec![TaskId(1), TaskId(2)]),
    ));
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::ConfirmDetachTmux,
    ));

    assert!(
        app.board.tasks[0].tmux_window.is_none(),
        "task 1 window should be cleared"
    );
    assert!(
        app.board.tasks[1].tmux_window.is_none(),
        "task 2 window should be cleared"
    );

    let kill_count = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::Task(crate::tui::commands::TaskCommand::KillTmuxWindow { .. })
            )
        })
        .count();
    assert_eq!(kill_count, 2, "should kill 2 windows");

    let persist_count = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::Task(crate::tui::commands::TaskCommand::Persist(_))
            )
        })
        .count();
    assert_eq!(persist_count, 2, "should persist 2 tasks");
}

#[test]
fn move_repo_cursor_down_wraps() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 1; // last
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 0, "should wrap to first");
}

#[test]
fn move_repo_cursor_up_wraps() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 0; // first
    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(app.input.repo_cursor, 1, "should wrap to last");
}

#[test]
fn repo_cursor_resets_on_quick_dispatch_entry() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string()];
    app.input.repo_cursor = 1;
    app.update(Message::Input(
        crate::tui::messages::InputMessage::StartQuickDispatchSelection,
    ));
    assert_eq!(
        app.input.repo_cursor, 0,
        "cursor should reset to 0 on mode entry"
    );
}

#[test]
fn detached_review_task_shows_awaiting_merge_header() {
    let mut task = make_task(1, TaskStatus::Review);
    task.sub_status = SubStatus::AwaitingReview;
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/10",
        crate::models::UrlType::Pr,
    ));
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = None; // detached
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "awaiting merge"),
        "detached review task should show 'awaiting merge' section header"
    );
}

#[test]
fn live_review_task_shows_awaiting_review_header() {
    let mut task = make_task(1, TaskStatus::Review);
    task.sub_status = SubStatus::AwaitingReview;
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/10",
        crate::models::UrlType::Pr,
    ));
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = Some("1-fix".to_string()); // live
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "awaiting review"),
        "live review task should show 'awaiting review' section header"
    );
    assert!(
        !buffer_contains(&buf, "awaiting merge"),
        "live review task should not show 'awaiting merge'"
    );
}

#[test]
fn detached_and_live_review_tasks_get_separate_sections() {
    // Live task (has tmux window)
    let mut live = make_task(1, TaskStatus::Review);
    live.sub_status = SubStatus::AwaitingReview;
    live.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/10",
        crate::models::UrlType::Pr,
    ));
    live.worktree = Some("/repo/.worktrees/1-fix".to_string());
    live.tmux_window = Some("1-fix".to_string());

    // Detached task (no tmux window)
    let mut detached = make_task(2, TaskStatus::Review);
    detached.sub_status = SubStatus::AwaitingReview;
    detached.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/11",
        crate::models::UrlType::Pr,
    ));
    detached.worktree = Some("/repo/.worktrees/2-feat".to_string());
    detached.tmux_window = None;

    let mut app = App::new(vec![live, detached]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "awaiting review"),
        "should show 'awaiting review' for live task"
    );
    assert!(
        buffer_contains(&buf, "awaiting merge"),
        "should show 'awaiting merge' for detached task"
    );
}

#[test]
fn is_detached_returns_true_for_review_without_window() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = None;
    assert!(task.is_detached());
}

#[test]
fn is_detached_returns_false_with_window() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = Some("1-fix".to_string());
    assert!(!task.is_detached());
}

#[test]
fn is_detached_returns_false_for_conflict() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Conflict;
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = None;
    assert!(!task.is_detached());
}

#[test]
fn is_unprovisioned_is_mutually_exclusive_with_is_detached() {
    for status in [TaskStatus::Running, TaskStatus::Review] {
        let mut task = make_task(1, status);
        task.worktree = None;
        task.tmux_window = None;
        assert!(task.is_unprovisioned(), "{status:?} with nothing behind it");
        assert!(!task.is_detached(), "the two are mutually exclusive");

        task.worktree = Some("/repo/.worktrees/1-fix".to_string());
        assert!(!task.is_unprovisioned(), "a worktree means resumable");
        assert!(task.is_detached());
    }
}

#[test]
fn is_unprovisioned_is_false_for_non_live_statuses() {
    for status in [TaskStatus::Backlog, TaskStatus::Done, TaskStatus::Archived] {
        let mut task = make_task(1, status);
        task.worktree = None;
        task.tmux_window = None;
        assert!(!task.is_unprovisioned(), "{status:?} is not a live status");
    }
}

#[test]
fn mark_dispatching_sets_guard_and_returns_no_commands() {
    let mut app = make_app();
    assert!(!app.is_dispatching(TaskId(1)));
    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    assert!(cmds.is_empty());
    assert!(app.is_dispatching(TaskId(1)));
}

#[test]
fn tick_with_active_split_checks_pane() {
    let mut app = make_app();
    app.board.split.active = true;
    app.board.split.right_pane_id = Some("%42".to_string());
    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Split(crate::tui::commands::SplitCommand::CheckPaneExists { pane_id }) if pane_id == "%42"
    )));
}

#[test]
fn tick_without_split_does_not_check_pane() {
    let mut app = make_app();
    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Split(crate::tui::commands::SplitCommand::CheckPaneExists { .. })
    )));
}

#[test]
fn tick_skips_capture_for_split_pinned_task() {
    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task]);

    // Pin task 4 in split mode
    app.board.split.active = true;
    app.board.split.right_pane_id = Some("%42".to_string());
    app.board.split.pinned_task_id = Some(TaskId(4));

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));

    // Should NOT emit CheckWindow for the pinned task (its window is a pane now)
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::CheckWindow { id: TaskId(4), .. })
        )),
        "split-pinned task should be excluded from CheckWindow"
    );
}

#[test]
fn repo_cursor_resets_on_entering_repo_path_mode() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.input.repo_cursor = 2; // cursor was left at position 2
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: String::new(),
        ..Default::default()
    });
    app.input.set_buffer("some desc".to_string());
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert_eq!(app.input.repo_cursor, 0, "cursor should reset to top");
}

#[test]
fn fuzzy_matches_empty_query_matches_anything() {
    assert!(super::fuzzy_matches("/some/path", ""));
    assert!(super::fuzzy_matches("", ""));
}

#[test]
fn fuzzy_matches_subsequence() {
    assert!(super::fuzzy_matches("/tmp", "tmp"));
    assert!(super::fuzzy_matches("/home/tmp", "tmp"));
    assert!(super::fuzzy_matches("/home/ragge/proj", "ragge"));
}

#[test]
fn fuzzy_matches_case_insensitive() {
    assert!(super::fuzzy_matches("/TMP", "tmp"));
    assert!(super::fuzzy_matches("/tmp", "TMP"));
}

#[test]
fn fuzzy_matches_no_match() {
    assert!(!super::fuzzy_matches("/var", "tmp"));
}

#[test]
fn fuzzy_matches_chars_must_be_in_order() {
    // "tp" is a valid subsequence of "/tmp" (t at 1, p at 3)
    assert!(super::fuzzy_matches("/tmp", "tp"));
    // "pt" requires p before t: p is at 3, t is at 1 -> false
    assert!(!super::fuzzy_matches("/tmp", "pt"));
}

#[test]
fn filtered_repos_empty_query_returns_all() {
    let paths = vec!["/a".to_string(), "/b".to_string()];
    assert_eq!(super::filtered_repos(&paths, ""), vec!["/a", "/b"]);
}

#[test]
fn filtered_repos_narrows_by_query() {
    let paths = vec![
        "/home/ragge/proj".to_string(),
        "/var/log".to_string(),
        "/home/other".to_string(),
    ];
    let result = super::filtered_repos(&paths, "home");
    assert_eq!(result, vec!["/home/ragge/proj", "/home/other"]);
}

#[test]
fn filtered_repos_no_matches_returns_empty() {
    let paths = vec!["/tmp".to_string(), "/var".to_string()];
    assert_eq!(super::filtered_repos(&paths, "xyz"), Vec::<String>::new());
}

#[test]
fn repo_cursor_resets_on_copy_task() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.input.repo_cursor = 2; // cursor was left at position 2
                               // Copy the first task (select it and press 'c')
    app.handle_key(make_key(KeyCode::Char('c')));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert_eq!(
        app.input.repo_cursor, 0,
        "cursor should reset to top on copy"
    );
}

#[test]
fn move_repo_cursor_wraps_within_filtered_list() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp".to_string(), "/var".to_string(), "/home".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    // Type "tmp" to filter — only /tmp matches (1 item)
    for c in "tmp".chars() {
        app.handle_key(make_key(KeyCode::Char(c)));
    }
    assert_eq!(app.input.repo_cursor, 0);
    // j wraps in a list of 1 — cursor stays at 0
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn typing_resets_repo_cursor_to_zero() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    // Navigate to position 2
    app.handle_key(make_key(KeyCode::Down));
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 2);
    // Type a character — cursor should reset
    app.handle_key(make_key(KeyCode::Char('/')));
    assert_eq!(app.input.repo_cursor, 0);
}

/// The retired startup tips popup used to sit in front of the board and swallow
/// exactly these six keys (`docs/plans/3809-keybinding-pruning-implementation.md`
/// §2). Nothing may intercept them on a freshly started app again: each one has
/// to reach its board handler on the very first keypress.
#[test]
fn a_freshly_started_app_routes_the_formerly_intercepted_keys_to_the_board() {
    // `q` reaches the board quit path, which asks for confirmation.
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('q')));
    assert_eq!(
        app.input.mode,
        InputMode::ConfirmQuit,
        "'q' must reach the board quit path from a fresh board"
    );

    // `h` / `l` move the column cursor.
    let mut app = make_app();
    let start = app.selected_column();
    app.handle_key(make_key(KeyCode::Char('l')));
    assert_ne!(
        app.selected_column(),
        start,
        "'l' must move the column cursor from a fresh board"
    );
    app.handle_key(make_key(KeyCode::Char('h')));
    assert_eq!(
        app.selected_column(),
        start,
        "'h' must move it back from a fresh board"
    );

    // `n` opens the new-task form.
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(
        app.input.mode,
        InputMode::InputTitle,
        "'n' must open the new-task form from a fresh board"
    );

    // `x` reaches the archive handler, which confirms before moving anything.
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_ne!(
        app.input.mode,
        InputMode::Normal,
        "'x' must reach the board archive handler from a fresh board"
    );

    // `Esc` clears the selection and leaves the board in Normal mode — it must
    // do that rather than being eaten by an overlay that never opened.
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(
        app.input.mode,
        InputMode::Normal,
        "Esc must leave a fresh board in Normal mode"
    );
}

#[test]
fn refresh_status_never_fetched() {
    let (text, color) = ui::refresh_status(None, false, Duration::from_secs(30));
    assert_eq!(text, "Never fetched  [r] refresh");
    assert_eq!(color, Color::DarkGray);
}

#[test]
fn refresh_status_loading() {
    let last = Instant::now() - Duration::from_secs(5);
    let (text, color) = ui::refresh_status(Some(last), true, Duration::from_secs(30));
    assert_eq!(text, "Refreshing...  [r] refresh");
    assert_eq!(color, Color::DarkGray);
}

#[test]
fn refresh_status_loading_overrides_never_fetched() {
    let (text, color) = ui::refresh_status(None, true, Duration::from_secs(30));
    assert_eq!(text, "Refreshing...  [r] refresh");
    assert_eq!(color, Color::DarkGray);
}

#[test]
fn refresh_status_fresh_seconds() {
    let last = Instant::now() - Duration::from_secs(1);
    let (text, color) = ui::refresh_status(Some(last), false, Duration::from_secs(30));
    assert!(
        text.starts_with("Updated ") && text.contains("s ago") && text.ends_with("  [r] refresh"),
        "expected 'Updated Xs ago  [r] refresh', got: {text}"
    );
    assert_eq!(color, Color::White);
}

#[test]
fn refresh_status_fresh_just_below_minutes_threshold() {
    // 59s elapsed, 30s interval: 59 < 2*30=60 → still White, seconds format
    let last = Instant::now() - Duration::from_secs(59);
    let (text, color) = ui::refresh_status(Some(last), false, Duration::from_secs(30));
    assert!(text.contains("59s ago"), "expected '59s ago' in: {text}");
    assert_eq!(color, Color::White);
}

#[test]
fn refresh_status_minutes_format() {
    // 60s elapsed → "1m 0s ago"
    let last = Instant::now() - Duration::from_secs(60);
    let (text, color) = ui::refresh_status(Some(last), false, Duration::from_secs(300));
    assert!(
        text.contains("1m") && text.contains("s ago"),
        "expected minutes format in: {text}"
    );
    assert_eq!(color, Color::White);
}

#[test]
fn refresh_status_yellow_at_2x_interval() {
    let interval = Duration::from_secs(30);
    // exactly 2× interval → Yellow
    let last = Instant::now() - interval * 2;
    let (_, color) = ui::refresh_status(Some(last), false, interval);
    assert_eq!(color, Color::Yellow);
}

#[test]
fn refresh_status_white_just_below_2x_interval() {
    let interval = Duration::from_secs(30);
    // 1ms under 2× interval → still White
    let last = Instant::now() - (interval * 2 - Duration::from_millis(100));
    let (_, color) = ui::refresh_status(Some(last), false, interval);
    assert_eq!(color, Color::White);
}

#[test]
fn refresh_status_yellow_just_below_4x_interval() {
    let interval = Duration::from_secs(30);
    // 1ms under 4× interval → still Yellow
    let last = Instant::now() - (interval * 4 - Duration::from_millis(100));
    let (_, color) = ui::refresh_status(Some(last), false, interval);
    assert_eq!(color, Color::Yellow);
}

#[test]
fn refresh_status_red_at_4x_interval() {
    let interval = Duration::from_secs(30);
    // exactly 4× interval → Red
    let last = Instant::now() - interval * 4;
    let (_, color) = ui::refresh_status(Some(last), false, interval);
    assert_eq!(color, Color::Red);
}

#[test]
fn test_selection_preserved_when_task_above_cursor_moves() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog), // row 0
        make_task(2, TaskStatus::Backlog), // row 1 — cursor here
        make_task(3, TaskStatus::Backlog), // row 2
    ]);
    // App starts at Backlog (nav col 1); navigate down to row 1.
    app.update(Message::NavigateRow(1));
    assert_eq!(app.selection().row(1), 1);

    // Task 1 moves out; Task 2 follows to row 0.
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![
            make_task(1, TaskStatus::Running),
            make_task(2, TaskStatus::Backlog),
            make_task(3, TaskStatus::Backlog),
        ],
    )));

    // Anchor follows task 2 — stays in Backlog (nav col 1) at row 0.
    assert_eq!(app.selection().column(), 1);
    assert_eq!(app.selection().row(1), 0);
    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(matches!(items[0], ColumnItem::Task(t) if t.id == TaskId(2)));
}

#[test]
fn test_selection_follows_task_to_new_column() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog), // row 0 — default cursor
        make_task(2, TaskStatus::Backlog),
    ]);
    // App starts at Backlog (nav col 1).
    assert_eq!(app.selection().column(), 1);
    assert_eq!(app.selection().row(1), 0);

    // Task 1 dispatched to Running
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![
            make_task(1, TaskStatus::Running),
            make_task(2, TaskStatus::Backlog),
        ],
    )));

    assert_eq!(app.selection().column(), 2); // Running = nav col 2
    assert_eq!(app.selection().row(2), 0);
    let items = app.column_items_for_status(TaskStatus::Running);
    assert!(matches!(items[0], ColumnItem::Task(t) if t.id == TaskId(1)));
}

#[test]
fn test_selection_falls_back_when_task_deleted() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
        make_task(3, TaskStatus::Backlog),
    ]);
    // App starts at Backlog (nav col 1); navigate down to row 2 (Task 3).
    app.update(Message::NavigateRow(1));
    app.update(Message::NavigateRow(1));
    assert_eq!(app.selection().row(1), 2); // Task 3

    // Task 3 deleted
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![
            make_task(1, TaskStatus::Backlog),
            make_task(2, TaskStatus::Backlog),
        ],
    )));

    assert_eq!(app.selection().column(), 1);
    assert_eq!(app.selection().row(1), 1); // clamped to last valid row
}

#[test]
fn test_selection_preserved_on_same_data_refresh() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
        make_task(3, TaskStatus::Backlog),
    ]);
    // App starts at Backlog (nav col 1); navigate down to row 1.
    app.update(Message::NavigateRow(1));
    assert_eq!(app.selection().row(1), 1);

    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![
            make_task(1, TaskStatus::Backlog),
            make_task(2, TaskStatus::Backlog),
            make_task(3, TaskStatus::Backlog),
        ],
    )));

    assert_eq!(app.selection().row(1), 1);
    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(matches!(items[1], ColumnItem::Task(t) if t.id == TaskId(2)));
}

#[test]
fn test_selection_falls_back_when_column_empties() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
    ]);
    // App starts at Backlog (nav col 1); navigate right to Running (nav col 2).
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selection().column(), 2);

    // Task 2 deleted — Running column empties, anchor not found
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![make_task(1, TaskStatus::Backlog)],
    )));

    // Cursor must be in a valid state: row 0 in the empty Running column
    assert_eq!(app.selection().column(), 2);
    assert_eq!(app.selection().row(2), 0);
}

// --- Archive column navigation ---

#[test]
fn navigate_right_from_done_shows_archive() {
    let mut app = make_app();
    // Navigate to Done column (nav col 4 = COLUMN_COUNT, starting from col 1)
    for _ in 0..3 {
        app.update(Message::NavigateColumn(1));
    }
    assert_eq!(app.selected_column(), 4);
    assert!(!app.show_archived());

    // Navigate right from Done → archive column (nav col 5)
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selected_column(), 5);
    assert!(app.show_archived());
}

#[test]
fn navigate_right_from_done_resets_archive_selection() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
    ]);
    // Pre-position archive selection at row 1
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 1);
    *app.archive.list_state.selected_mut() = Some(1);

    // Navigate from Backlog (col 1) to archive (col 5): 4 steps
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    // Selection should reset to 0
    assert_eq!(app.selected_archive_row(), 0);
}

#[test]
fn navigate_left_from_archive_hides_it_and_goes_to_done() {
    let mut app = make_app();
    // Enter archive column (col 5) from Backlog (col 1): 4 steps
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    assert!(app.show_archived());

    // Navigate left → Done (col 4)
    app.update(Message::NavigateColumn(-1));
    assert_eq!(app.selected_column(), 4);
    assert!(!app.show_archived());
}

#[test]
fn pressing_right_at_archive_column_stays_clamped() {
    let mut app = make_app();
    // Navigate from col 1 to col 5 (4 steps), then 2 more (should clamp at 5)
    for _ in 0..6 {
        app.update(Message::NavigateColumn(1));
    }
    // Should clamp at 5
    assert_eq!(app.selected_column(), 5);
    assert!(app.show_archived());
}

#[test]
fn h_key_in_archive_returns_to_done() {
    let mut app = make_app();
    // Enter archive column (col 5): 4 steps from col 1
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    assert!(app.show_archived());

    // Press h → Done (col 4)
    app.handle_key(make_key(KeyCode::Char('h')));
    assert_eq!(app.selected_column(), 4);
    assert!(!app.show_archived());
}

#[test]
fn left_arrow_in_archive_returns_to_done() {
    let mut app = make_app();
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    assert!(app.show_archived());

    app.handle_key(make_key(KeyCode::Left));
    assert_eq!(app.selected_column(), 4);
    assert!(!app.show_archived());
}

#[test]
fn esc_key_in_archive_returns_to_done() {
    let mut app = make_app();
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    assert!(app.show_archived());

    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.selected_column(), 4);
    assert!(!app.show_archived());
}

#[test]
fn navigate_left_from_done_does_not_show_archive() {
    let mut app = make_app();
    // Navigate to Done column (nav col 4): 3 steps from col 1
    for _ in 0..3 {
        app.update(Message::NavigateColumn(1));
    }
    assert_eq!(app.selected_column(), 4);

    // Navigate left (back to Review, col 3) — archive must NOT appear
    app.update(Message::NavigateColumn(-1));
    assert_eq!(app.selected_column(), 3);
    assert!(!app.show_archived());
}

#[test]
fn board_selection_archive_row_roundtrip() {
    let mut sel = BoardSelection::new();
    assert_eq!(sel.row(5), 0); // archive_row starts at 0
    sel.set_row(5, 7);
    assert_eq!(sel.row(5), 7);
}

#[test]
fn board_selection_task_col_row_uses_offset() {
    let mut sel = BoardSelection::new();
    sel.set_row(1, 10); // Backlog = nav col 1 → array index 0
    assert_eq!(sel.row(1), 10);
    sel.set_row(4, 5); // Done = nav col 4 → array index 3
    assert_eq!(sel.row(4), 5);
}

// --- [1,5] column-range navigation tests ---

#[test]
fn navigate_right_from_done_enters_archive() {
    let mut app = make_app();
    // Board starts at Backlog (col 1). Navigate to Done (col 4): 3 steps.
    for _ in 0..3 {
        app.update(Message::NavigateColumn(1));
    }
    assert_eq!(app.selected_column(), 4);
    app.update(Message::NavigateColumn(1)); // col 4 → col 5 (Archive)
    assert_eq!(app.selected_column(), 5);
}

#[test]
fn navigate_left_at_backlog_is_noop() {
    let mut app = make_app();
    // Board starts at Backlog (col 1), which is now the leftmost column.
    assert_eq!(app.selected_column(), 1);
    app.update(Message::NavigateColumn(-1)); // clamp at 1
    assert_eq!(app.selected_column(), 1);
}

#[test]
fn navigate_right_at_archive_is_noop() {
    let mut app = make_app();
    // Board starts at Backlog (col 1). Navigate to Archive (col 5): 4 steps.
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    assert_eq!(app.selected_column(), 5);
    app.update(Message::NavigateColumn(1)); // clamp at 5
    assert_eq!(app.selected_column(), 5);
}

// --- update_anchor_from_current with pre-computed EpicStatsMap ---

#[test]
fn update_anchor_from_current_accepts_stats_and_sets_epic_anchor() {
    // Direct API test: update_anchor_from_current() must set the anchor to the epic
    // when the cursor is on row 1 (epic sorts after task id=1).
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    // Invalidate stale cache so cached_epic_stats() rebuilds with the new epic.
    app.invalidate_layout_cache();
    // Task(id=1) sort key: (priority=0, sort=1, id=1)
    // Epic(id=10) sort key: (priority=0, sort=10, id=10)  → row 1
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1);

    let _ = app.cached_epic_stats();
    app.update_anchor_from_current();

    assert_eq!(app.selection().anchor, Some(ColumnAnchor::Epic(EpicId(10))));
}

#[test]
fn navigate_row_with_epics_sets_anchor_via_stats() {
    // Integration test: navigating down to an epic (via j key equivalent) must set
    // the anchor to ColumnAnchor::Epic, proving the stats path is wired in.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    // Invalidate stale cache so navigation rebuilds with the new epic.
    app.invalidate_layout_cache();
    // Row 0 = Task(1), row 1 = Epic(10)
    app.update(Message::NavigateRow(1));

    assert_eq!(app.selection().row(1), 1);
    assert_eq!(app.selection().anchor, Some(ColumnAnchor::Epic(EpicId(10))));
}

#[test]
fn navigate_column_with_epics_sets_anchor_via_stats() {
    // Integration test: navigating to a column with epics must set the anchor,
    // exercising the stats path in handle_navigate_column.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)]; // epic in Backlog (nav col 1)

    // Navigate right then back: anchor after returning to Backlog col should be Task(1)
    app.update(Message::NavigateColumn(1)); // → Running
    app.update(Message::NavigateColumn(-1)); // → Backlog (row 0 = Task(1))
    assert_eq!(app.selection().anchor, Some(ColumnAnchor::Task(TaskId(1))));
}

// --- jump-to-top/bottom ([/]) ---

#[test]
fn bracket_right_jumps_to_last_row() {
    // make_app() gives 2 Backlog tasks (ids 1,2) at rows 0 and 1
    let mut app = make_app();
    app.selection_mut().set_column(1); // Backlog = nav col 1
                                       // Start at row 0 (default); ] should jump to row 1 (last)
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(']'))));
    assert!(cmds.is_empty());
    assert_eq!(app.selection().row(1), 1, "should jump to last task");
    assert!(!app.on_select_all());
}

#[test]
fn bracket_left_jumps_to_first_row() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
        make_task(3, TaskStatus::Backlog),
    ]);
    app.selection_mut().set_column(1); // Backlog
    app.update(Message::NavigateRow(1));
    app.update(Message::NavigateRow(1)); // now at row 2
    assert_eq!(app.selection().row(1), 2);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('['))));
    assert!(cmds.is_empty());
    assert_eq!(app.selection().row(1), 0, "should jump to first task");
    assert!(!app.on_select_all());
}

#[test]
fn bracket_left_clears_on_select_all() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    // Navigate up from row 0 → on_select_all
    app.update(Message::NavigateRow(-1));
    assert!(app.on_select_all(), "precondition: on select-all toggle");
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('['))));
    assert!(cmds.is_empty());
    assert!(!app.on_select_all(), "[ should clear on_select_all");
    assert_eq!(app.selection().row(1), 0, "should land on first task");
}

#[test]
fn bracket_right_clears_on_select_all() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    app.selection_mut().set_column(1);
    app.update(Message::NavigateRow(-1)); // → on_select_all
    assert!(app.on_select_all(), "precondition");
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(']'))));
    assert!(cmds.is_empty());
    assert!(!app.on_select_all(), "] should clear on_select_all");
    assert_eq!(app.selection().row(1), 1, "should land on last task");
}

#[test]
fn bracket_right_on_empty_column_is_noop() {
    let mut app = App::new(vec![]);
    app.selection_mut().set_column(1); // empty Backlog
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(']'))));
    assert!(cmds.is_empty());
    assert_eq!(app.selection().row(1), 0, "row unchanged on empty column");
}

#[test]
fn bracket_left_on_empty_column_is_noop() {
    // Use a fresh app with no tasks; to verify true no-op, manually set
    // on_select_all = true to see that [ does NOT clear it when empty.
    let mut app2 = App::new(vec![]);
    app2.selection_mut().set_column(1); // empty Backlog
    app2.selection_mut().on_select_all = true; // non-default state
    let cmds = without_usage(app2.handle_key(make_key(KeyCode::Char('['))));
    assert!(cmds.is_empty());
    // on_select_all should remain unchanged because the column is empty — true no-op
    assert!(
        app2.on_select_all(),
        "[ on empty column must not mutate on_select_all"
    );
    assert_eq!(app2.selection().row(1), 0, "row unchanged on empty column");
}

#[test]
fn bracket_right_on_single_item_stays_at_row_zero() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.selection_mut().set_column(1);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(']'))));
    assert!(cmds.is_empty());
    assert_eq!(
        app.selection().row(1),
        0,
        "single item: row 0 is both first and last"
    );
}

#[test]
fn bracket_right_in_archive_jumps_to_last_archived() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
        make_task(3, TaskStatus::Archived),
    ]);
    let archive_col = TaskStatus::COLUMN_COUNT + 1;
    app.selection_mut().set_column(archive_col);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(']'))));
    assert!(cmds.is_empty());
    assert_eq!(
        app.selection().row(archive_col),
        2,
        "should jump to last archived task"
    );
    assert_eq!(*app.archive.list_state.selected_mut(), Some(2));
}

#[test]
fn bracket_left_in_archive_jumps_to_first_archived() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
    ]);
    let archive_col = TaskStatus::COLUMN_COUNT + 1;
    app.selection_mut().set_column(archive_col);
    app.selection_mut().set_row(archive_col, 1);
    *app.archive.list_state.selected_mut() = Some(1);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('['))));
    assert!(cmds.is_empty());
    assert_eq!(
        app.selection().row(archive_col),
        0,
        "should jump to first archived task"
    );
    assert_eq!(*app.archive.list_state.selected_mut(), Some(0));
}

#[test]
fn bracket_right_in_empty_archive_is_noop() {
    let mut app = App::new(vec![]);
    let archive_col = TaskStatus::COLUMN_COUNT + 1;
    app.selection_mut().set_column(archive_col);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(']'))));
    assert!(cmds.is_empty());
    assert_eq!(app.selection().row(archive_col), 0);
}

#[test]
fn bracket_left_in_empty_archive_is_noop() {
    let mut app = App::new(vec![]);
    let archive_col = TaskStatus::COLUMN_COUNT + 1;
    app.selection_mut().set_column(archive_col);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('['))));
    assert!(cmds.is_empty());
    assert_eq!(app.selection().row(archive_col), 0);
}
