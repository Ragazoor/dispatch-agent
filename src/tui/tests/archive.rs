#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{EpicId, SubStatus, TaskId, TaskStatus};
use crossterm::event::KeyCode;

#[test]
fn x_key_on_done_task_enters_confirm_archive_mode() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Done)]);
    app.selection_mut().set_column(4); // Done (nav col 4)
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('x'))));
    assert!(cmds.is_empty());
    assert!(matches!(app.input.mode, InputMode::ConfirmArchive(Some(_))));
    assert_eq!(app.status.message.as_deref(), Some("Archive task? [y/n]"));
}

#[test]
fn confirm_archive_y_emits_archive_task() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Done)]);
    app.selection_mut().set_column(4); // Done (nav col 4)
    app.handle_key(make_key(KeyCode::Char('x')));
    let _ = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    // Task 1 should now be Archived
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
}

#[test]
fn x_key_on_backlog_task_enters_confirm_done_not_archive() {
    let mut app = make_app();
    // make_app() starts at Backlog (nav col 1) — no need to set_column.
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('x'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmDone);
    assert_eq!(app.select.pending_done, vec![TaskId(1)]);
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Backlog,
        "task should not move until confirmed"
    );
}

#[test]
fn confirm_x_on_backlog_task_moves_to_done_not_archived() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('x')));
    app.handle_key(make_key(KeyCode::Char('y')));
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
}

#[test]
fn x_key_on_running_task_moves_to_done_and_preserves_worktree() {
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = Some("/wt/1-test".to_string());
    task.tmux_window = Some("dev:1-test".to_string());
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(2); // Running (nav col 2)

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmDone);
    assert_eq!(app.select.pending_done, vec![TaskId(1)]);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('y'))));

    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(
        task.worktree.as_deref(),
        Some("/wt/1-test"),
        "moving to Done must not remove the worktree"
    );
    assert!(task.tmux_window.is_none(), "tmux window should be killed");
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
        )),
        "moving to Done must not emit worktree Cleanup"
    );
}

#[test]
fn x_key_on_review_task_enters_confirm_done_not_archive() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)]);
    app.selection_mut().set_column(3); // Review (nav col 3)
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('x'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmDone);
    assert_eq!(app.select.pending_done, vec![TaskId(1)]);
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "task should not move until confirmed"
    );
}

#[test]
fn confirm_x_on_review_task_y_moves_to_done_not_archived() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)]);
    app.selection_mut().set_column(3); // Review (nav col 3)
    app.handle_key(make_key(KeyCode::Char('x')));
    app.handle_key(make_key(KeyCode::Char('y')));
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
}

#[test]
fn x_key_on_all_review_selection_enters_confirm_done_not_archive() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
        make_task(2, TaskStatus::Review),
    ]);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    app.handle_key(make_key(KeyCode::Char('x')));

    assert_eq!(
        app.input.mode,
        InputMode::ConfirmDone,
        "expected ConfirmDone, got {:?}",
        app.input.mode
    );
    app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Done);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Done);
}

#[test]
fn x_key_on_mixed_status_selection_moves_non_done_to_done() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Done),
    ]);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(
        app.input.mode,
        InputMode::ConfirmDone,
        "expected ConfirmDone, got {:?}",
        app.input.mode
    );
    app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Done);
    assert_eq!(
        app.find_task(TaskId(2)).unwrap().status,
        TaskStatus::Done,
        "the already-Done task in the selection is left untouched"
    );
}

#[test]
fn x_key_on_all_done_selection_archives() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
    ]);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    app.handle_key(make_key(KeyCode::Char('x')));
    assert!(matches!(app.input.mode, InputMode::ConfirmArchive(None)));
    app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(
        app.find_task(TaskId(1)).unwrap().status,
        TaskStatus::Archived
    );
    assert_eq!(
        app.find_task(TaskId(2)).unwrap().status,
        TaskStatus::Archived
    );
}

#[test]
fn confirm_archive_n_cancels() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Done)]);
    app.selection_mut().set_column(4); // Done (nav col 4)
    app.handle_key(make_key(KeyCode::Char('x')));
    let _ = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    // Task 1 still in Done, not archived
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
}

#[test]
fn confirm_done_n_cancels_and_leaves_task_in_place() {
    let mut app = make_app();
    // make_app() starts at Backlog (nav col 1).
    app.handle_key(make_key(KeyCode::Char('x')));
    let _ = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
}

#[test]
fn archive_targets_task_at_x_press_not_at_y_press() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
        make_task(3, TaskStatus::Done),
    ]);
    // Navigate to Done column (nav col 4) and move down to task 2 (row 1).
    app.selection_mut().set_column(4);
    app.update(Message::NavigateRow(1));
    assert_eq!(app.selection().row(4), 1);

    // Press 'x' — cursor is on task 2.
    app.handle_key(make_key(KeyCode::Char('x')));

    // Simulate a background refresh where task 2 (the one we wanted to archive)
    // was archived externally.  sync_board_selection cannot find the anchor
    // (task 2 is now Archived and excluded from visible columns), so it clamps.
    // The Done column now contains only task 3 at row 0 — the cursor drifts
    // there.
    let mut t2_archived = make_task(2, TaskStatus::Done);
    t2_archived.status = TaskStatus::Archived;
    let refreshed = vec![
        make_task(1, TaskStatus::Done),
        t2_archived,
        make_task(3, TaskStatus::Done),
    ];
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        refreshed,
    )));
    // After the refresh the Done column is [task 1, task 3]; the cursor clamped
    // to row 1 (task 3, the last visible item).
    assert_eq!(
        app.selected_column(),
        4,
        "cursor should still be in Done column"
    );

    // Press 'y'.  Task 2 was already archived externally, so archiving it again
    // is a no-op.  What must NOT happen is task 3 being archived instead —
    // that would mean the handler used the (drifted) cursor row instead of the
    // task ID that was captured when 'x' was pressed.
    app.handle_key(make_key(KeyCode::Char('y')));
    assert_ne!(
        app.find_task(TaskId(3)).unwrap().status,
        TaskStatus::Archived,
        "task 3 should NOT be archived — cursor drifted to it after 'x'"
    );
}

#[test]
fn archive_task_sets_status_and_emits_persist() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Done)]);
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Archive(
        TaskId(1),
    )));
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[test]
fn archive_task_with_worktree_emits_cleanup() {
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = Some("/wt/1-test".to_string());
    task.tmux_window = Some("dev:1-test".to_string());
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Archive(
        TaskId(1),
    )));

    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
}

#[test]
fn archive_task_without_worktree_no_cleanup() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Archive(
        TaskId(1),
    )));
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[test]
fn archive_clears_agent_tracking() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("dev:1-test".to_string());
    task.sub_status = SubStatus::Stale;
    let mut app = App::new(vec![task]);
    app.agents.notified_review.insert(TaskId(1));
    app.agents.notified_needs_input.insert(TaskId(1));
    app.agents
        .message_flash
        .insert(TaskId(1), std::time::Instant::now());
    app.agents
        .last_pr_poll
        .insert(TaskId(1), std::time::Instant::now());

    app.update(Message::Task(crate::tui::messages::TaskMessage::Archive(
        TaskId(1),
    )));

    assert!(!app.agents.notified_review.contains(&TaskId(1)));
    assert!(!app.agents.notified_needs_input.contains(&TaskId(1)));
    assert!(!app.agents.message_flash.contains_key(&TaskId(1)));
    assert!(!app.agents.last_pr_poll.contains_key(&TaskId(1)));
}

#[test]
fn archive_panel_j_k_navigation() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
        make_task(3, TaskStatus::Archived),
    ]);
    app.selection_mut().set_column(5);
    assert_eq!(app.selected_archive_row(), 0);

    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.selected_archive_row(), 1);

    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.selected_archive_row(), 2);

    // Clamp at end
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.selected_archive_row(), 2);

    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.selected_archive_row(), 1);
}

#[test]
fn archive_panel_x_enters_confirm_delete() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Archived)]);
    app.selection_mut().set_column(5);

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmDelete);
    assert_eq!(
        app.status.message.as_deref(),
        Some("Delete \"Task 1\"? [y/n]")
    );
}

#[test]
fn archive_panel_confirm_delete_removes_task() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Archived)]);
    app.selection_mut().set_column(5);

    app.handle_key(make_key(KeyCode::Char('x')));
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert!(app.board.tasks.is_empty());
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Delete(TaskId(1)))
    )));
}

#[test]
fn archived_tasks_not_in_kanban_columns() {
    let app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Archived),
    ]);

    for &status in TaskStatus::ALL {
        let tasks = app.tasks_by_status(status);
        for t in &tasks {
            assert_ne!(
                t.status,
                TaskStatus::Archived,
                "archived task should not appear in {} column",
                status.as_str()
            );
        }
    }

    let archived = app.archived_tasks();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, TaskId(2));
}

#[test]
fn full_archive_flow() {
    // A Done task still holding its worktree — 'x' archives from Done.
    let mut task = make_task(1, TaskStatus::Done);
    task.worktree = Some("/wt/1-test".to_string());
    task.tmux_window = Some("dev:1-test".to_string());
    let mut app = App::new(vec![task, make_task(2, TaskStatus::Backlog)]);

    // Navigate to the Done column (nav col 4)
    app.selection_mut().set_column(4);

    // Press x to archive
    app.handle_key(make_key(KeyCode::Char('x')));
    assert!(matches!(app.input.mode, InputMode::ConfirmArchive(Some(_))));

    // Confirm
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);

    // Task should be archived with cleanup
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
    assert!(task.worktree.is_none());
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
    )));

    // Navigate to archive column (directly set to col 5 — the archive nav column)
    app.selection_mut().set_column(5);
    assert!(app.show_archived());

    // Should see 1 archived task
    assert_eq!(app.archived_tasks().len(), 1);

    // Hard delete from archive
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmDelete);

    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Delete(TaskId(1)))
    )));
    assert!(app.archived_tasks().is_empty());
}

#[test]
fn batch_archive_archives_all_and_clears_selection() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
        make_task(3, TaskStatus::Backlog),
    ]);

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::BatchArchive(vec![TaskId(1), TaskId(2)]),
    ));

    assert_eq!(
        app.find_task(TaskId(1)).unwrap().status,
        TaskStatus::Archived
    );
    assert_eq!(
        app.find_task(TaskId(2)).unwrap().status,
        TaskStatus::Archived
    );
    assert_eq!(
        app.find_task(TaskId(3)).unwrap().status,
        TaskStatus::Backlog
    );
    // Selection should be cleared after archive
    assert!(app.select.tasks.is_empty());
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
fn confirm_archive_with_selection_dispatches_batch() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
    ]);

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));
    app.input.mode = InputMode::ConfirmArchive(None);

    app.handle_key(make_key(KeyCode::Char('y')));

    assert_eq!(
        app.find_task(TaskId(1)).unwrap().status,
        TaskStatus::Archived
    );
    assert_eq!(
        app.find_task(TaskId(2)).unwrap().status,
        TaskStatus::Archived
    );
    assert!(app.select.tasks.is_empty());
}

#[test]
fn render_archive_overlay_shows_archived_tasks() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.status = TaskStatus::Archived;
    task.title = "Archived Item".to_string();
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(5);
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        buffer_contains(&buf, "Archived Item"),
        "archive overlay should show archived task title"
    );
}

#[test]
fn x_key_on_epic_enters_confirm_archive_epic() {
    let mut app = make_app_with_epic_selected();
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('x'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmArchiveEpic);
    assert!(app
        .status
        .message
        .as_deref()
        .unwrap()
        .contains("Archive epic"));
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
    ]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.board.epics = vec![epic];
    // Subtasks are hidden in board view. Epic status is Running (nav col 2).
    // Epic is the only item in Running column → row 0.
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('x'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app
        .status
        .message
        .as_deref()
        .unwrap()
        .contains("Cannot archive epic"));
    assert!(app
        .status
        .message
        .as_deref()
        .unwrap()
        .contains("2 subtasks not done"));
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
    ]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.board.epics = vec![epic];
    // 2 Done + 1 Running → epic status Running (nav col 2). Epic is only item → row 0.
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('x'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app
        .status
        .message
        .as_deref()
        .unwrap()
        .contains("1 subtask not done"));
}

#[test]
fn x_key_on_epic_with_all_done_subtasks_allows_archive() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Done);
        t.epic_id = Some(EpicId(10));
        t
    }]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Done;
    app.board.epics = vec![epic];
    // All done → epic status Done (nav col 4). Epic is only item → row 0.
    app.selection_mut().set_column(4);
    app.selection_mut().set_row(4, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('x'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmArchiveEpic);
    assert!(app
        .status
        .message
        .as_deref()
        .unwrap()
        .contains("Archive epic"));
}

#[test]
fn confirm_archive_epic_no_subtasks_allows_archive() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    // No subtasks → derived status Backlog (nav col 1). Epic is only item → row 0.
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let cmds = app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ConfirmArchive,
    ));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmArchiveEpic);
    assert!(app
        .status
        .message
        .as_deref()
        .unwrap()
        .contains("Archive epic"));
}

#[test]
fn confirm_archive_epic_y_archives() {
    let mut app = make_app_confirm_archive_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status.message.is_none());
    // Soft archive: epic stays in memory with status = Archived
    assert_eq!(app.board.epics.len(), 1);
    assert_eq!(app.board.epics[0].status, TaskStatus::Archived);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist {
            id,
            status: Some(TaskStatus::Archived),
            ..
        }) if *id == EpicId(10)
    )));
    // Must NOT emit DeleteEpic — that path triggers FK violations.
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Delete(_))
    )));
}

#[test]
fn confirm_archive_epic_uppercase_y_archives() {
    let mut app = make_app_confirm_archive_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('Y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert_eq!(app.board.epics.len(), 1);
    assert_eq!(app.board.epics[0].status, TaskStatus::Archived);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist {
            id,
            status: Some(TaskStatus::Archived),
            ..
        }) if *id == EpicId(10)
    )));
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Delete(_))
    )));
}

#[test]
fn archive_epic_archives_subtasks_and_persists_each() {
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
    ]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Done;
    app.board.epics = vec![epic];

    let cmds = app.update(Message::Epic(crate::tui::messages::EpicMessage::Archive(
        EpicId(10),
    )));

    // Both subtasks now archived in memory
    assert!(app
        .board
        .tasks
        .iter()
        .all(|t| t.status == TaskStatus::Archived));
    // PersistTask emitted for each subtask
    let persist_task_ids: Vec<_> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::Task(crate::tui::commands::TaskCommand::Persist(t)) => Some(t.id),
            _ => None,
        })
        .collect();
    assert!(persist_task_ids.contains(&TaskId(1)));
    assert!(persist_task_ids.contains(&TaskId(2)));
    // Epic itself archived and persisted
    assert_eq!(app.board.epics[0].status, TaskStatus::Archived);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist {
            id,
            status: Some(TaskStatus::Archived),
            ..
        }) if *id == EpicId(10)
    )));
}

#[test]
fn archive_epic_recursively_archives_sub_epics_and_their_tasks() {
    // Parent epic 10 has child epic 20; child epic has one Done task.
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Done);
        t.epic_id = Some(EpicId(20));
        t
    }]);
    let mut parent = make_epic(10);
    parent.status = TaskStatus::Backlog;
    let mut child = make_epic(20);
    child.parent_epic_id = Some(EpicId(10));
    child.status = TaskStatus::Done;
    app.board.epics = vec![parent, child];

    let cmds = app.update(Message::Epic(crate::tui::messages::EpicMessage::Archive(
        EpicId(10),
    )));

    // Both epics now archived
    let parent_status = app
        .board
        .epics
        .iter()
        .find(|e| e.id == EpicId(10))
        .unwrap()
        .status;
    let child_status = app
        .board
        .epics
        .iter()
        .find(|e| e.id == EpicId(20))
        .unwrap()
        .status;
    assert_eq!(parent_status, TaskStatus::Archived);
    assert_eq!(child_status, TaskStatus::Archived);

    // Subtask of child epic also archived
    assert_eq!(app.board.tasks[0].status, TaskStatus::Archived);

    // PersistEpic emitted for both parent and child
    let persisted_epic_ids: Vec<_> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::Epic(crate::tui::commands::EpicCommand::Persist {
                id,
                status: Some(TaskStatus::Archived),
                ..
            }) => Some(*id),
            _ => None,
        })
        .collect();
    assert!(persisted_epic_ids.contains(&EpicId(10)));
    assert!(persisted_epic_ids.contains(&EpicId(20)));

    // No DeleteEpic emitted on the archive path.
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Delete(_))
    )));
}

#[test]
fn confirm_archive_epic_other_key_cancels() {
    let mut app = make_app_confirm_archive_epic();
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status.message.is_none());
    assert_eq!(app.board.epics.len(), 1); // not removed
    assert!(cmds.is_empty());
}

#[test]
fn confirm_archive_epic_no_epic_selected_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.selection_mut().set_column(1); // Backlog = nav col 1
    app.input.mode = InputMode::ConfirmArchiveEpic;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('y'))));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty());
}

#[test]
fn archive_panel_down_arrow_navigates() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
    ]);
    app.selection_mut().set_column(5);
    assert_eq!(app.selected_archive_row(), 0);
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.selected_archive_row(), 1);
}

#[test]
fn archive_panel_up_arrow_navigates() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
    ]);
    app.selection_mut().set_column(5);
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 1);
    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(app.selected_archive_row(), 0);
}

#[test]
fn archive_panel_esc_closes() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Archived)]);
    app.selection_mut().set_column(5);
    app.handle_key(make_key(KeyCode::Esc));
    assert!(!app.show_archived());
}

#[test]
fn archive_panel_e_edits_task() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Archived)]);
    app.selection_mut().set_column(5);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('e'))));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Editor(crate::tui::commands::EditorCommand::PopOut(EditKind::TaskEdit(t))) if t.id == TaskId(1))
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn archive_panel_e_on_empty_is_noop() {
    let mut app = App::new(vec![]);
    app.selection_mut().set_column(5);
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(cmds.is_empty());
}

#[test]
fn archive_panel_x_on_empty_is_noop() {
    let mut app = App::new(vec![]);
    app.selection_mut().set_column(5);
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::Normal); // did not enter ConfirmDelete
}

#[test]
fn archive_panel_q_enters_confirm_quit() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Archived)]);
    app.selection_mut().set_column(5);
    app.handle_key(make_key(KeyCode::Char('q')));
    assert!(!app.should_quit);
    assert_eq!(app.input.mode, InputMode::ConfirmQuit);
}

#[test]
fn archive_panel_unrecognized_key_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Archived)]);
    app.selection_mut().set_column(5);
    let cmds = app.handle_key(make_key(KeyCode::Char('z')));
    assert!(cmds.is_empty());
    assert!(app.show_archived());
}

#[test]
fn confirm_archive_uppercase_y_archives() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmArchive(Some(TaskId(1)));
    app.handle_key(make_key(KeyCode::Char('Y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
}

#[test]
fn confirm_archive_esc_cancels() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmArchive(Some(TaskId(1)));
    app.status.message = Some("Archive task? [y/n]".to_string());
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status.message.is_none());
    assert!(cmds.is_empty());
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Backlog); // unchanged
}

#[test]
fn repo_filter_applies_to_archived_tasks() {
    let mut app = App::new(vec![]);
    let mut t1 = make_task(1, TaskStatus::Archived);
    t1.repo_path = "/repo-a".to_string();
    let mut t2 = make_task(2, TaskStatus::Archived);
    t2.repo_path = "/repo-b".to_string();
    app.board.tasks = vec![t1, t2];
    app.filter.repos.insert("/repo-a".to_string());

    let archived = app.archived_tasks();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, TaskId(1));
}

#[test]
fn repo_filter_exclude_applies_to_archived() {
    let mut app = App::new(vec![]);
    let mut t1 = make_task(1, TaskStatus::Archived);
    t1.repo_path = "/repo-a".to_string();
    let mut t2 = make_task(2, TaskStatus::Archived);
    t2.repo_path = "/repo-b".to_string();
    app.board.tasks = vec![t1, t2];
    app.filter.repos.insert("/repo-a".to_string());
    app.filter.mode = RepoFilterMode::Exclude;

    let archived = app.archived_tasks();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, TaskId(2));
}

#[test]
fn handle_key_confirm_archive_yes() {
    let mut app = make_app();
    // Select task 1 (backlog)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);
    app.input.mode = InputMode::ConfirmArchive(Some(TaskId(1)));

    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(*app.mode(), InputMode::Normal);
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Persist(t)) if t.status == TaskStatus::Archived)));
}

#[test]
fn handle_key_confirm_archive_cancel() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmArchive(Some(TaskId(1)));

    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn batch_archive_selected_epics() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];

    let cmds = app.update(Message::Epic(
        crate::tui::messages::EpicMessage::BatchArchive(vec![EpicId(10), EpicId(20)]),
    ));
    // Soft archive: epics stay in memory with status = Archived
    assert_eq!(app.board.epics.len(), 2);
    assert!(app
        .board
        .epics
        .iter()
        .all(|e| e.status == TaskStatus::Archived));
    assert!(!cmds.is_empty(), "Should emit commands");
}

#[test]
fn batch_archive_skips_epics_with_non_done_subtasks() {
    let mut task = make_task(1, TaskStatus::Running);
    task.epic_id = Some(EpicId(10));
    let mut app = App::new(vec![task]);
    app.board.epics = vec![make_epic(10)];

    let cmds = app.update(Message::Epic(
        crate::tui::messages::EpicMessage::BatchArchive(vec![EpicId(10)]),
    ));
    assert_eq!(
        app.board.epics.len(),
        1,
        "Epic with non-done subtask should not be archived"
    );
    assert!(cmds.is_empty(), "Should not emit commands for skipped epic");
}

#[test]
fn batch_archive_mixed_tasks_and_epics() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(10)),
    ));

    app.handle_key(make_key(KeyCode::Char('x')));
    assert!(matches!(app.input.mode, InputMode::ConfirmArchive(None)));
    assert_eq!(
        app.status.message.as_deref(),
        Some("Archive 2 items? [y/n]")
    );

    // Confirm
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(
        app.find_task(TaskId(1)).unwrap().status,
        TaskStatus::Archived
    );
    // Soft archive: epic stays in memory with status = Archived
    assert_eq!(app.board.epics.len(), 1);
    assert_eq!(app.board.epics[0].status, TaskStatus::Archived);
    assert!(app.select.tasks.is_empty());
    assert!(app.select.epics.is_empty());
    assert!(!cmds.is_empty());
}

#[test]
fn confirm_archive_y_archives_selected_epics() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(10)),
    ));
    app.input.mode = InputMode::ConfirmArchive(None);

    app.handle_key(make_key(KeyCode::Char('y')));
    // Soft archive: epic stays in memory with status = Archived
    assert_eq!(app.board.epics.len(), 1);
    assert_eq!(app.board.epics[0].status, TaskStatus::Archived);
    assert!(app.select.epics.is_empty());
}

#[test]
fn render_status_bar_confirm_archive() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmArchive(Some(TaskId(1)));
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Archive task?"),
        "ConfirmArchive should show 'Archive task?'"
    );
}

#[test]
fn render_status_bar_confirm_archive_epic() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmArchiveEpic;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Archive epic"),
        "ConfirmArchiveEpic should show 'Archive epic'"
    );
}

#[test]
fn archive_esc_closes_overlay() {
    let mut app = make_app();
    // Archive a task first
    app.update(Message::Task(crate::tui::messages::TaskMessage::Archive(
        TaskId(1),
    )));
    app.selection_mut().set_column(5);
    app.handle_key(make_key(KeyCode::Esc));
    assert!(!app.show_archived());
}

#[test]
fn archive_e_directly_edits_task() {
    let mut app = make_app();
    app.update(Message::Task(crate::tui::messages::TaskMessage::Archive(
        TaskId(1),
    )));
    app.selection_mut().set_column(5);
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Editor(crate::tui::commands::EditorCommand::PopOut(
            EditKind::TaskEdit(_)
        ))
    )));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn archive_q_quits() {
    let mut app = make_app();
    app.update(Message::Task(crate::tui::messages::TaskMessage::Archive(
        TaskId(1),
    )));
    app.selection_mut().set_column(5);
    app.handle_key(make_key(KeyCode::Char('q')));
    assert_eq!(app.input.mode, InputMode::ConfirmQuit);
}

#[test]
fn handle_key_archive_j_navigates_down() {
    let mut app = make_app();
    // Add archived tasks
    let mut t1 = make_task(100, TaskStatus::Archived);
    t1.title = "Archived 1".to_string();
    let mut t2 = make_task(101, TaskStatus::Archived);
    t2.title = "Archived 2".to_string();
    app.board.tasks.push(t1);
    app.board.tasks.push(t2);
    app.selection_mut().set_column(5);
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 0);

    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.selected_archive_row(), 1);
}

#[test]
fn handle_key_archive_k_navigates_up() {
    let mut app = make_app();
    let mut t1 = make_task(100, TaskStatus::Archived);
    t1.title = "Archived 1".to_string();
    let mut t2 = make_task(101, TaskStatus::Archived);
    t2.title = "Archived 2".to_string();
    app.board.tasks.push(t1);
    app.board.tasks.push(t2);
    app.selection_mut().set_column(5);
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 1);

    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.selected_archive_row(), 0);
}

#[test]
fn handle_key_archive_k_clamps_at_zero() {
    let mut app = make_app();
    let t = make_task(100, TaskStatus::Archived);
    app.board.tasks.push(t);
    app.selection_mut().set_column(5);
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 0);

    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.selected_archive_row(), 0);
}

#[test]
fn handle_key_archive_down_arrow_navigates() {
    let mut app = make_app();
    let t1 = make_task(100, TaskStatus::Archived);
    let t2 = make_task(101, TaskStatus::Archived);
    app.board.tasks.push(t1);
    app.board.tasks.push(t2);
    app.selection_mut().set_column(5);
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 0);

    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.selected_archive_row(), 1);
}

#[test]
fn handle_key_archive_up_arrow_navigates() {
    let mut app = make_app();
    let t1 = make_task(100, TaskStatus::Archived);
    let t2 = make_task(101, TaskStatus::Archived);
    app.board.tasks.push(t1);
    app.board.tasks.push(t2);
    app.selection_mut().set_column(5);
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 1);

    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(app.selected_archive_row(), 0);
}

#[test]
fn handle_key_archive_x_enters_confirm_delete() {
    let mut app = make_app();
    let t = make_task(100, TaskStatus::Archived);
    app.board.tasks.push(t);
    app.selection_mut().set_column(5);
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 0);

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(*app.mode(), InputMode::ConfirmDelete);
}

#[test]
fn handle_key_archive_e_directly_edits() {
    let mut app = make_app();
    let t = make_task(100, TaskStatus::Archived);
    app.board.tasks.push(t);
    app.selection_mut().set_column(5);
    app.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Editor(crate::tui::commands::EditorCommand::PopOut(
            EditKind::TaskEdit(t)
        )) if t.id == TaskId(100)
    )));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_archive_esc_closes() {
    let mut app = make_app();
    app.selection_mut().set_column(5);
    app.handle_key(make_key(KeyCode::Esc));
    assert!(!app.show_archived());
}

#[test]
fn handle_key_archive_q_quits() {
    let mut app = make_app();
    app.selection_mut().set_column(5);
    app.handle_key(make_key(KeyCode::Char('q')));
    assert_eq!(*app.mode(), InputMode::ConfirmQuit);
}

#[test]
fn handle_key_archive_unknown_key_is_noop() {
    let mut app = make_app();
    app.selection_mut().set_column(5);
    let cmds = app.handle_key(make_key(KeyCode::Char('z')));
    assert!(cmds.is_empty());
}

/// ConfirmArchive mode routes to the confirm-archive handler.
#[test]
fn handle_key_confirm_archive_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmArchive(None);
    // 'n' cancels
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn archive_column_renders_task_cards_when_focused() {
    let mut app = make_app_with_archived_task();
    // Navigate to Archive (col 5)
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    assert_eq!(app.selected_column(), TaskStatus::COLUMN_COUNT + 1);
    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "archived task"),
        "expected archived task card in buffer"
    );
}

/// ConfirmArchiveEpic mode routes correctly.
#[test]
fn handle_key_confirm_archive_epic_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmArchiveEpic;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}
