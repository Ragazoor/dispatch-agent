#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{EpicId, SubStatus, TaskId, TaskStatus};
use crossterm::event::KeyCode;

#[test]
fn finish_complete_moves_to_done() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }]);

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::FinishComplete(TaskId(1)),
    ));
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    // Worktree is preserved — will be cleaned up during archive
    assert!(task.worktree.is_some());
    assert!(task.tmux_window.is_none());
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[test]
fn finish_failed_with_conflict_sets_flag() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }]);

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::FinishFailed {
            id: TaskId(1),
            error: "Rebase conflict".to_string(),
            is_conflict: true,
        },
    ));
    assert!(app
        .find_task(TaskId(1))
        .is_some_and(|t| t.sub_status == SubStatus::Conflict));
    assert!(app
        .status
        .message
        .as_ref()
        .unwrap()
        .contains("Rebase conflict"));
}

#[test]
fn finish_failed_without_conflict_does_not_set_flag() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }]);

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::FinishFailed {
            id: TaskId(1),
            error: "Not on main".to_string(),
            is_conflict: false,
        },
    ));
    assert!(!app
        .find_task(TaskId(1))
        .is_some_and(|t| t.sub_status == SubStatus::Conflict));
}

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
fn status_bar_shows_wrap_up_hint_for_review_task() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    let mut app = App::new(vec![task]);
    // Navigate to Review column (index 2)
    for _ in 0..2 {
        app.update(Message::NavigateColumn(1));
    }

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "[W]rap up"),
        "Status bar should show wrap up hint for Review tasks"
    );
}

#[test]
fn w_key_on_review_task_with_worktree_enters_wrap_up() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }]);
    // Navigate to Review column (index 2)
    app.update(Message::NavigateColumn(2));

    app.handle_key(make_key(KeyCode::Char('W')));
    assert!(matches!(
        app.input.mode,
        InputMode::ConfirmWrapUp(TaskId(1))
    ));
}

#[test]
fn wrap_up_r_emits_finish_command() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }]);
    app.update(Message::NavigateColumn(4));

    app.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Start(
        TaskId(1),
    )));
    let cmds = app.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Rebase));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Finish { .. })
    )));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn wrap_up_p_emits_no_command_and_points_at_skill() {
    // PR creation is agent-driven now (see the /wrap-up skill).
    // Pressing `p` in ConfirmWrapUp must not dispatch a PR command;
    // it just exits the prompt and shows a hint.
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }]);
    app.update(Message::NavigateColumn(4));

    app.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Start(
        TaskId(1),
    )));
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(1));
    let cmds = app.handle_key(make_key(KeyCode::Char('p')));
    assert!(
        cmds.is_empty(),
        "p should not dispatch any command; got: {cmds:?}"
    );
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(
        app.status
            .message
            .as_deref()
            .unwrap_or("")
            .contains("/wrap-up"),
        "status should point the user at the /wrap-up skill; got: {:?}",
        app.status.message
    );
}

#[test]
fn wrap_up_esc_cancels() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }]);
    app.update(Message::NavigateColumn(4));

    app.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Start(
        TaskId(1),
    )));
    app.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Cancel));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn wrap_up_rebase_clears_conflict_flag() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }]);

    app.find_task_mut(TaskId(1)).unwrap().sub_status = SubStatus::Conflict;
    app.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Start(
        TaskId(1),
    )));
    app.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Rebase));
    assert!(!app
        .find_task(TaskId(1))
        .is_some_and(|t| t.sub_status == SubStatus::Conflict));
}

#[test]
fn wrap_up_available_on_running_blocked() {
    let mut app = make_app();
    let id = TaskId(3); // Running
    app.find_task_mut(id).unwrap().sub_status = SubStatus::NeedsInput;
    app.find_task_mut(id).unwrap().worktree = Some("/tmp/wt".to_string());
    app.selection_mut().set_column(3); // Blocked column
    app.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Start(
        id,
    )));
    assert!(matches!(app.mode(), InputMode::ConfirmWrapUp(_)));
}

#[test]
fn wrap_up_available_on_running_active() {
    let mut app = make_app();
    let id = TaskId(3); // Running, Active by default
    app.find_task_mut(id).unwrap().worktree = Some("/tmp/wt".to_string());
    app.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Start(
        id,
    )));
    assert!(matches!(app.mode(), InputMode::ConfirmWrapUp(_)));
}

#[test]
fn w_key_on_epic_starts_epic_wrap_up() {
    let mut app = App::new(vec![make_review_subtask(1, 10, 1)]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Review;
    app.board.epics = vec![epic];
    // Epic is in Review column (column 2)
    app.selection_mut().set_column(3);
    app.selection_mut().set_row(3, 0);

    app.handle_key(make_key(KeyCode::Char('W')));

    assert!(matches!(app.input.mode, InputMode::ConfirmEpicWrapUp(_)));
}

#[test]
fn epic_wrap_up_with_review_tasks_enters_confirm() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 1),
        make_review_subtask(2, 10, 2),
    ]);
    app.board.epics = vec![make_epic(10)];

    app.update(Message::WrapUp(
        crate::tui::messages::WrapUpMessage::EpicStart(EpicId(10)),
    ));

    assert!(matches!(
        app.input.mode,
        InputMode::ConfirmEpicWrapUp(EpicId(10))
    ));
}

#[test]
fn epic_wrap_up_without_review_tasks_shows_info() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.epic_id = Some(EpicId(10));
    let mut app = App::new(vec![task]);
    app.board.epics = vec![make_epic(10)];

    app.update(Message::WrapUp(
        crate::tui::messages::WrapUpMessage::EpicStart(EpicId(10)),
    ));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app
        .status
        .message
        .as_ref()
        .unwrap()
        .contains("No review tasks"));
}

#[test]
fn epic_wrap_up_rebase_creates_queue_and_emits_first_finish() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 2),
        make_review_subtask(2, 10, 1),
    ]);
    app.board.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(10));

    let cmds = app.update(Message::WrapUp(
        crate::tui::messages::WrapUpMessage::EpicRebase,
    ));

    assert_eq!(app.input.mode, InputMode::Normal);
    let queue = app.merge_queue.as_ref().expect("merge queue should exist");
    // Task 2 has sort_order 1, so it comes first
    assert_eq!(queue.task_ids, vec![TaskId(2), TaskId(1)]);
    assert_eq!(queue.current, Some(TaskId(2)));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Finish { id, .. }) if *id == TaskId(2))));
}

#[test]
fn epic_wrap_up_finish_complete_advances_queue() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 2),
        make_review_subtask(2, 10, 1),
    ]);
    app.board.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(10));
    app.update(Message::WrapUp(
        crate::tui::messages::WrapUpMessage::EpicRebase,
    ));

    // First task completes
    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::FinishComplete(TaskId(2)),
    ));

    let queue = app.merge_queue.as_ref().expect("queue should still exist");
    assert_eq!(queue.completed, 1);
    assert_eq!(queue.current, Some(TaskId(1)));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Finish { id, .. }) if *id == TaskId(1))));
}

#[test]
fn epic_wrap_up_all_complete_clears_queue() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 2),
        make_review_subtask(2, 10, 1),
    ]);
    app.board.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(10));
    app.update(Message::WrapUp(
        crate::tui::messages::WrapUpMessage::EpicRebase,
    ));

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::FinishComplete(TaskId(2)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::FinishComplete(TaskId(1)),
    ));

    assert!(
        app.merge_queue.is_none(),
        "queue should be cleared after all tasks complete"
    );
}

#[test]
fn epic_wrap_up_finish_failed_pauses_queue() {
    let mut app = App::new(vec![
        make_review_subtask(1, 10, 2),
        make_review_subtask(2, 10, 1),
    ]);
    app.board.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(10));
    app.update(Message::WrapUp(
        crate::tui::messages::WrapUpMessage::EpicRebase,
    ));

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::FinishFailed {
            id: TaskId(2),
            error: "rebase conflict".to_string(),
            is_conflict: true,
        },
    ));

    let queue = app.merge_queue.as_ref().expect("queue should still exist");
    assert_eq!(queue.failed, Some(TaskId(2)));
    assert!(queue.current.is_none());
}

#[test]
fn epic_wrap_up_cancel_clears_queue() {
    let mut app = App::new(vec![make_review_subtask(1, 10, 1)]);
    app.board.epics = vec![make_epic(10)];
    app.merge_queue = Some(MergeQueue {
        epic_id: EpicId(10),
        task_ids: vec![TaskId(1)],
        completed: 0,
        current: Some(TaskId(1)),
        failed: None,
    });

    app.update(Message::WrapUp(
        crate::tui::messages::WrapUpMessage::CancelMergeQueue,
    ));

    assert!(app.merge_queue.is_none());
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
fn handle_key_confirm_wrap_up_rebase() {
    let mut app = make_app();
    let mut task = make_task(10, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/10-test".to_string());
    task.tmux_window = Some("main:10-test".to_string());
    app.board.tasks.push(task);
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(10));

    let cmds = app.handle_key(make_key(KeyCode::Char('r')));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Finish { id, .. }) if *id == TaskId(10))));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_wrap_up_pr_no_longer_dispatches() {
    // PR creation is agent-driven; the TUI shortcut emits no command.
    let mut app = make_app();
    let mut task = make_task(10, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/10-test".to_string());
    task.tmux_window = Some("main:10-test".to_string());
    app.board.tasks.push(task);
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(10));

    let cmds = app.handle_key(make_key(KeyCode::Char('p')));
    assert!(cmds.is_empty(), "p should not dispatch a command anymore");
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
fn render_status_bar_confirm_done() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDone;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Done?"),
        "ConfirmDone should show 'Done?'"
    );
}

#[test]
fn render_status_bar_confirm_wrap_up() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(1));
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "rebase"),
        "ConfirmWrapUp should show 'rebase'"
    );
}

#[test]
fn render_status_bar_confirm_epic_wrap_up() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(1));
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Epic wrap up"),
        "ConfirmEpicWrapUp should show 'Epic wrap up'"
    );
}

#[test]
fn confirm_epic_wrap_up_r_sends_rebase() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(1));
    let cmds = app.handle_key(make_key(KeyCode::Char('r')));
    // Should produce an effect related to epic wrap-up rebase
    assert!(!cmds.is_empty() || app.input.mode == InputMode::Normal);
}

#[test]
fn confirm_epic_wrap_up_p_no_longer_dispatches() {
    // Epic-merge PR batching is gone — the same defect as W+p
    // (auto-generated bodies). Pressing `p` exits the prompt with a
    // hint pointing at the agent /wrap-up flow.
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(1));
    let cmds = app.handle_key(make_key(KeyCode::Char('p')));
    assert!(cmds.is_empty(), "epic-merge p should not dispatch");
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_epic_wrap_up_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(1));
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_epic_wrap_up_unknown_key_is_noop() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(1));
    let cmds = app.handle_key(make_key(KeyCode::Char('z')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmEpicWrapUp(EpicId(1)));
}

#[test]
fn handle_key_normal_wrap_up_task() {
    let mut task = make_task(10, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/10-test".to_string());
    task.tmux_window = Some("main:10-test".to_string());
    let mut app = App::new(vec![task]);
    // Select the review column
    app.selection_mut().set_column(3);
    app.selection_mut().set_row(3, 0);
    app.handle_key(make_key(KeyCode::Char('W')));
    assert!(matches!(*app.mode(), InputMode::ConfirmWrapUp(TaskId(10))));
}

#[test]
fn handle_key_normal_wrap_up_epic() {
    let mut subtask = make_task(20, TaskStatus::Review);
    subtask.epic_id = Some(EpicId(10));
    subtask.worktree = Some("/repo/.worktrees/20-test".to_string());
    let mut app = App::new(vec![subtask]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Review;
    app.board.epics = vec![epic];
    // Epic is in Review column
    app.selection_mut().set_column(3);
    app.selection_mut().set_row(3, 0);
    app.handle_key(make_key(KeyCode::Char('W')));
    assert!(matches!(
        *app.mode(),
        InputMode::ConfirmEpicWrapUp(EpicId(10))
    ));
}

#[test]
fn handle_key_normal_wrap_up_on_empty_is_noop() {
    let mut app = make_app();
    // Navigate to an empty column (Review has no tasks by default)
    app.selection_mut().set_column(3);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('W'))));
    assert!(cmds.is_empty());
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

/// ConfirmWrapUp mode routes correctly.
#[test]
fn handle_key_confirm_wrap_up_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(1));
    // Esc cancels wrap-up
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// ConfirmEpicWrapUp mode routes correctly.
#[test]
fn handle_key_confirm_epic_wrap_up_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmEpicWrapUp(EpicId(1));
    // Esc cancels epic wrap-up
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_wrap_up_d_marks_task_done_no_git_command() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Running);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }]);
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(1));

    let cmds = app.handle_key(make_key(KeyCode::Char('d')));

    // Task should be marked Done in-memory
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done, "task should be marked Done");

    // Should emit a Persist command (no Finish/git command)
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Persist(_))
        )),
        "expected Persist command, got {:?}",
        cmds
    );
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Finish { .. })
        )),
        "should NOT emit Finish (no git ops)"
    );

    // Mode should return to Normal
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_wrap_up_d_kills_tmux_window_and_clears_reference() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Running);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }]);
    app.input.mode = InputMode::ConfirmWrapUp(TaskId(1));

    let cmds = app.handle_key(make_key(KeyCode::Char('d')));

    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::KillTmuxWindow { .. })
        )),
        "expected KillTmuxWindow command, got {:?}",
        cmds
    );
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert!(
        task.tmux_window.is_none(),
        "tmux_window should be cleared on the task"
    );
    // Worktree is preserved — will be cleaned up during archive
    assert!(task.worktree.is_some());
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
