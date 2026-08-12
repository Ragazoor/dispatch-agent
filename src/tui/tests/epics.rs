#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{EpicId, SubStatus, TaskId, TaskStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};

#[test]
fn epic_edited_splices_all_fields_including_feed_settings() {
    // Regression guard: handle_epic_edited previously only copied
    // title/description/updated_at into the in-memory board row, dropping
    // feed_command/feed_interval_secs until the next DB refresh (~10s). In
    // that window the 'r' feed-refresh key checks the stale in-memory value.
    let mut app = App::new(vec![]);
    app.board.epics.push(make_epic(1));

    let mut edited = make_epic(1);
    edited.title = "New Title".to_string();
    edited.description = "New description".to_string();
    edited.feed_command = Some("gh api repos/x/pulls".to_string());
    edited.feed_interval_secs = Some(300);

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Edited(
        edited.clone(),
    )));

    let spliced = app.board.epics.iter().find(|e| e.id == EpicId(1)).unwrap();
    assert_eq!(spliced.feed_command, edited.feed_command);
    assert_eq!(spliced.feed_interval_secs, edited.feed_interval_secs);
    assert_eq!(spliced.title, "New Title");
    assert_eq!(spliced.description, "New description");
}

#[test]
fn toggle_flattened_message_flips_state() {
    let mut app = App::new(vec![]);
    assert!(!app.board.flattened);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleFlattened,
    ));
    assert!(app.board.flattened);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleFlattened,
    ));
    assert!(!app.board.flattened);
}

#[test]
fn epic_action_hints_not_done() {
    let epic = make_epic(1);
    let hints = ui::epic_action_hints(&epic, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"[Enter]"), "epic shows detail");
    assert!(keys.contains(&"[L]"), "epic shows status forward");
    assert!(keys.contains(&"[H]"), "epic shows status backward");
    assert!(keys.contains(&"[x]"), "epic shows archive");
}

#[test]
fn epic_action_hints_done() {
    let mut epic = make_epic(1);
    epic.status = TaskStatus::Done;
    let hints = ui::epic_action_hints(&epic, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"[L]"), "done epic shows status forward");
    assert!(keys.contains(&"[H]"), "done epic shows status backward");
}

#[test]
fn action_hints_no_ctrl_g_outside_epic() {
    let task = make_task(1, TaskStatus::Backlog);
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys = hint_keys(&hints);
    assert!(
        !keys.contains(&"[^g]"),
        "should not show ^g back outside epic view"
    );
}

#[test]
fn epic_action_hints_shows_filter_help() {
    let epic = make_epic(1);
    let hints = ui::epic_action_hints(&epic, Color::Rgb(122, 162, 247));
    let keys = hint_keys(&hints);
    assert!(keys.contains(&"[f]"), "epic should show filter hint");
    assert!(keys.contains(&"[?]"), "epic should show help hint");
}

#[test]
fn description_editor_result_for_epic() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputEpicDescription;
    app.input.epic_draft = Some(EpicDraft {
        title: "E".to_string(),
        ..Default::default()
    });
    let cmds = app.update(Message::Editor(
        crate::tui::messages::EditorMessage::DescriptionResult("epic desc\nline 2".to_string()),
    ));
    // Description submit now completes creation immediately
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Insert(_))
    )));
}

#[test]
fn tasks_for_current_view_board_excludes_epic_tasks() {
    let mut app = App::new(vec![]);
    let standalone = make_task(1, TaskStatus::Backlog);
    let mut subtask = make_task(2, TaskStatus::Backlog);
    subtask.epic_id = Some(EpicId(10));
    app.board.tasks = vec![standalone, subtask];

    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, TaskId(1));
}

#[test]
fn tasks_for_current_view_epic_shows_only_subtasks() {
    let mut app = App::new(vec![]);
    let standalone = make_task(1, TaskStatus::Backlog);
    let mut subtask = make_task(2, TaskStatus::Running);
    subtask.epic_id = Some(EpicId(10));
    app.board.tasks = vec![standalone, subtask];

    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };

    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, TaskId(2));
}

#[test]
fn flattened_board_shows_running_subtasks_but_not_backlog() {
    let mut app = App::new(vec![]);
    let standalone = make_task(1, TaskStatus::Backlog);
    let mut backlog_subtask = make_task(2, TaskStatus::Backlog);
    backlog_subtask.epic_id = Some(EpicId(10));
    let mut running_subtask = make_task(3, TaskStatus::Running);
    running_subtask.epic_id = Some(EpicId(10));
    app.board.tasks = vec![standalone, backlog_subtask, running_subtask];
    app.board.epics = vec![make_epic(10)];
    app.board.flattened = true;

    let visible = app.tasks_for_current_view();
    let ids: std::collections::HashSet<_> = visible.iter().map(|t| t.id).collect();
    // Standalone backlog task is always visible
    assert!(ids.contains(&TaskId(1)));
    // Running subtask surfaces in flat mode
    assert!(ids.contains(&TaskId(3)));
    // Backlog subtask does NOT surface — backlog is excluded from flattening
    assert!(!ids.contains(&TaskId(2)));
}

#[test]
fn flattened_board_is_recursive_through_nested_epics() {
    let mut app = App::new(vec![]);
    // Epic tree: root(10) -> child(20)
    let mut child_epic = make_epic(20);
    child_epic.parent_epic_id = Some(EpicId(10));
    app.board.epics = vec![make_epic(10), child_epic];

    let mut t_root = make_task(1, TaskStatus::Backlog);
    t_root.epic_id = Some(EpicId(10));
    let mut t_leaf = make_task(2, TaskStatus::Running);
    t_leaf.epic_id = Some(EpicId(20));
    app.board.tasks = vec![t_root, t_leaf];
    app.board.flattened = true;

    // Backlog is NOT flattened: the root epic card shows, not the individual task
    let backlog = app.column_items_for_status(TaskStatus::Backlog);
    assert!(
        backlog
            .iter()
            .any(|i| matches!(i, ColumnItem::Epic(e) if e.id == EpicId(10))),
        "root epic card should appear in backlog in flat mode"
    );
    assert!(
        !backlog
            .iter()
            .any(|i| matches!(i, ColumnItem::Task(t) if t.id == TaskId(1))),
        "backlog subtask should NOT surface in flat mode"
    );

    // Running IS flattened: t_leaf bubbles up from the nested epic
    let running = app.column_items_for_status(TaskStatus::Running);
    assert!(running
        .iter()
        .any(|i| matches!(i, ColumnItem::Task(t) if t.id == TaskId(2))));
}

#[test]
fn flattened_board_hides_epic_cards_in_active_columns_but_not_backlog() {
    let mut app = App::new(vec![]);
    let mut child = make_epic(20);
    child.parent_epic_id = Some(EpicId(10));
    app.board.epics = vec![make_epic(10), child];
    app.board.flattened = true;

    // Running/Review/Done columns: epic cards are hidden (tasks surface via EpicHeader)
    for status in [TaskStatus::Running, TaskStatus::Review, TaskStatus::Done] {
        let items = app.column_items_for_status(status);
        assert!(
            items.iter().all(|i| matches!(
                i,
                ColumnItem::Task(_)
                    | ColumnItem::EpicHeader(_)
                    | ColumnItem::SubstatusLabel(_)
                    | ColumnItem::OrphanSeparator
            )),
            "flattened view should emit no navigable Epic cards in {status:?} column"
        );
    }

    // Backlog column: epic cards remain visible (backlog is excluded from flattening)
    let backlog_items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(
        backlog_items
            .iter()
            .any(|i| matches!(i, ColumnItem::Epic(_))),
        "backlog column should still show epic cards in flat mode"
    );
}

#[test]
fn flattened_epic_view_shows_only_that_subtree() {
    let mut app = App::new(vec![]);
    // Two root epics with tasks under each
    app.board.epics = vec![make_epic(10), make_epic(20)];
    let mut a = make_task(1, TaskStatus::Backlog);
    a.epic_id = Some(EpicId(10));
    let mut b = make_task(2, TaskStatus::Backlog);
    b.epic_id = Some(EpicId(20));
    app.board.tasks = vec![a, b];
    app.board.flattened = true;
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };

    let visible = app.tasks_for_current_view();
    let ids: std::collections::HashSet<_> = visible.iter().map(|t| t.id).collect();
    assert!(ids.contains(&TaskId(1)));
    assert!(!ids.contains(&TaskId(2)));
}

#[test]
fn shift_f_key_toggles_flattened() {
    let mut app = App::new(vec![]);
    assert!(!app.board.flattened);
    app.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    assert!(app.board.flattened);
    app.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    assert!(!app.board.flattened);
}

#[test]
fn shift_f_toggles_flattened_inside_epic_view() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));
    app.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    assert!(app.board.flattened);
}

#[test]
fn toggle_flattened_clamps_selection_in_backlog() {
    let mut app = App::new(vec![]);
    // Board with one root epic and one subtask inside. No standalone tasks.
    app.board.epics = vec![make_epic(10)];
    let mut subtask = make_task(1, TaskStatus::Backlog);
    subtask.epic_id = Some(EpicId(10));
    app.board.tasks = vec![subtask];

    // Select the (only) item in the backlog column: the epic card at row 0.
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    // Toggle flatten: backlog is excluded from flattening, so epic card stays.
    // Count stays 1 and row 0 remains valid.
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleFlattened,
    ));
    assert!(app.board.flattened);
    assert_eq!(app.selected_row()[0], 0);

    // Set row out-of-bounds, then toggle back to non-flat. Clamping still fires.
    app.selection_mut().set_row(1, 5);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleFlattened,
    ));
    assert!(!app.board.flattened);
    let count = app.column_items_for_status(TaskStatus::Backlog).len();
    assert!(count > 0);
    assert!(app.selected_row()[0] < count);
}

#[test]
fn flattened_survives_enter_and_exit_epic() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleFlattened,
    ));
    assert!(app.board.flattened);

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));
    assert!(app.board.flattened, "flatten should persist into epic view");

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Exit));
    assert!(app.board.flattened, "flatten should persist back to board");
}

#[test]
fn flattened_survives_refresh_tasks() {
    let mut app = App::new(vec![]);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleFlattened,
    ));
    assert!(app.board.flattened);

    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![make_task(1, TaskStatus::Backlog)],
    )));
    assert!(app.board.flattened);
}

#[test]
fn enter_on_epic_toggles_detail() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    // Epic is at row 0 in Backlog column (no standalone tasks)
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    app.handle_key(make_key(KeyCode::Enter));
    assert!(
        matches!(app.board.view_mode, ViewMode::Board(_)),
        "Should stay in board view — Enter on epic is a no-op until Task 5 input routing"
    );
}

#[test]
fn e_on_epic_opens_editor() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(
        matches!(&cmds[0], Command::Editor(crate::tui::commands::EditorCommand::PopOut(EditKind::EpicEdit(e))) if e.id == EpicId(10))
    );
}

#[test]
fn enter_epic_switches_to_epic_view() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(2);

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));

    match &app.board.view_mode {
        ViewMode::Epic {
            epic_id, parent, ..
        } => {
            assert_eq!(*epic_id, EpicId(10));
            match parent.as_ref() {
                ViewMode::Board(sel) => assert_eq!(sel.column(), 2, "board column should be saved"),
                _ => panic!("Expected parent to be ViewMode::Board"),
            }
        }
        _ => panic!("Expected ViewMode::Epic"),
    }
}

#[test]
fn exit_epic_restores_board_selection() {
    let mut app = App::new(vec![]);
    app.selection_mut().set_column(3);

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));
    app.selection_mut().set_column(1);

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Exit));

    match &app.board.view_mode {
        ViewMode::Board(sel) => {
            assert_eq!(sel.column(), 3, "board selection should be restored");
        }
        _ => panic!("Expected ViewMode::Board"),
    }
}

#[test]
fn exit_epic_when_on_board_is_noop() {
    let mut app = App::new(vec![]);
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Exit));
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn column_items_board_view_includes_epics() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)]; // epic with no subtasks = Backlog

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert_eq!(items.len(), 2); // 1 task + 1 epic
                                // Same priority (5), so task (id=1) sorts before epic (id=10)
    assert!(matches!(items[0], ColumnItem::Task(_)));
    assert!(matches!(items[1], ColumnItem::Epic(_)));
}

#[test]
fn column_items_epic_view_no_epics() {
    let mut app = App::new(vec![]);
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    app.board.epics = vec![make_epic(10)];

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(items.iter().all(|i| matches!(i, ColumnItem::Task(_))));
}

#[test]
fn selected_column_item_returns_epic() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];

    // Same priority (5), task (id=1) at row 0, epic (id=10) at row 1
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1);

    match app.selected_column_item() {
        Some(ColumnItem::Epic(e)) => assert_eq!(e.id, EpicId(10)),
        other => panic!("Expected Epic, got {:?}", other),
    }
}

#[test]
fn start_new_epic_sets_input_mode() {
    let mut app = make_app();
    app.update(Message::Epic(crate::tui::messages::EpicMessage::StartNew));
    assert_eq!(*app.mode(), InputMode::InputEpicTitle);
}

#[test]
fn epic_created_adds_to_state() {
    let mut app = App::new(vec![]);
    let epic = make_epic(1);
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Created(
        epic,
    )));
    assert_eq!(app.board.epics.len(), 1);
}

#[test]
fn delete_epic_removes_from_state_and_tasks() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    let mut subtask = make_task(1, TaskStatus::Backlog);
    subtask.epic_id = Some(EpicId(10));
    app.board.tasks = vec![subtask, make_task(2, TaskStatus::Backlog)];

    let cmds = app.update(Message::Epic(crate::tui::messages::EpicMessage::Delete(
        EpicId(10),
    )));
    assert!(app.board.epics.is_empty());
    assert_eq!(app.board.tasks.len(), 1);
    assert_eq!(app.board.tasks[0].id, TaskId(2));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Epic(crate::tui::commands::EpicCommand::Delete(id)) if *id == EpicId(10))));
}

#[test]
fn delete_epic_cleans_up_worktrees_of_sub_epic_subtasks() {
    // `EpicCrud::delete_epic` deletes the whole subtree (delete_epic_recursive
    // walks parent_epic_id), so a sub-epic's subtask rows vanish too. The
    // Cleanup commands must cover them, or their worktrees leak with no row
    // left to point at them. See DeleteEpic in docs/specs/epics.allium.
    let mut app = App::new(vec![]);
    let mut child_epic = make_epic(20);
    child_epic.parent_epic_id = Some(EpicId(10));
    app.board.epics = vec![make_epic(10), child_epic];

    let mut direct = make_task(1, TaskStatus::Running);
    direct.epic_id = Some(EpicId(10));
    direct.worktree = Some("/repo/.worktrees/1-direct".to_string());
    let mut nested = make_task(2, TaskStatus::Running);
    nested.epic_id = Some(EpicId(20));
    nested.worktree = Some("/repo/.worktrees/2-nested".to_string());
    app.board.tasks = vec![direct, nested];

    let cmds = app.update(Message::Epic(crate::tui::messages::EpicMessage::Delete(
        EpicId(10),
    )));

    let cleaned: Vec<&str> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::Task(crate::tui::commands::TaskCommand::Cleanup { worktree, .. }) => {
                Some(worktree.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(
        cleaned.contains(&"/repo/.worktrees/1-direct"),
        "direct subtask worktree must be cleaned up, got: {cleaned:?}"
    );
    assert!(
        cleaned.contains(&"/repo/.worktrees/2-nested"),
        "sub-epic subtask worktree must be cleaned up, got: {cleaned:?}"
    );
}

#[test]
fn delete_epic_removes_the_whole_subtree_from_the_board() {
    // The DB delete drops every descendant epic and its tasks, so the in-memory
    // board must not keep rows pointing at deleted epics.
    let mut app = App::new(vec![]);
    let mut child_epic = make_epic(20);
    child_epic.parent_epic_id = Some(EpicId(10));
    let mut grandchild_epic = make_epic(30);
    grandchild_epic.parent_epic_id = Some(EpicId(20));
    app.board.epics = vec![make_epic(10), child_epic, grandchild_epic, make_epic(40)];

    let mut nested = make_task(2, TaskStatus::Running);
    nested.epic_id = Some(EpicId(30));
    let mut unrelated = make_task(3, TaskStatus::Running);
    unrelated.epic_id = Some(EpicId(40));
    app.board.tasks = vec![nested, unrelated];

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Delete(
        EpicId(10),
    )));

    let epic_ids: Vec<EpicId> = app.board.epics.iter().map(|e| e.id).collect();
    assert_eq!(
        epic_ids,
        vec![EpicId(40)],
        "the deleted epic's whole subtree must leave the board"
    );
    let task_ids: Vec<TaskId> = app.board.tasks.iter().map(|t| t.id).collect();
    assert_eq!(
        task_ids,
        vec![TaskId(3)],
        "subtasks of descendant epics must leave the board too"
    );
}

#[test]
fn move_epic_status_forward() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)]; // starts as Backlog
    let cmds = app.update(Message::Epic(
        crate::tui::messages::EpicMessage::MoveStatus(EpicId(10), MoveDirection::Forward),
    ));
    assert_eq!(app.board.epics[0].status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist {
            id: EpicId(10),
            status: Some(TaskStatus::Running),
            ..
        })
    )));
}

#[test]
fn move_epic_status_backward() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Done;
    app.board.epics = vec![epic];
    let cmds = app.update(Message::Epic(
        crate::tui::messages::EpicMessage::MoveStatus(EpicId(10), MoveDirection::Backward),
    ));
    assert_eq!(app.board.epics[0].status, TaskStatus::Review);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist {
            id: EpicId(10),
            status: Some(TaskStatus::Review),
            ..
        })
    )));
}

#[test]
fn shift_l_key_on_epic_moves_status_forward() {
    let mut app = make_app_with_epic_selected();
    let cmds = app.handle_key(make_key(KeyCode::Char('L')));
    assert_eq!(app.board.epics[0].status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist { .. })
    )));
}

#[test]
fn shift_h_key_on_backlog_epic_stays_backlog() {
    let mut app = make_app_with_epic_selected();
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('H'))));
    // Already at Backlog, can't go backward
    assert_eq!(app.board.epics[0].status, TaskStatus::Backlog);
    assert!(cmds.is_empty());
}

#[test]
fn shift_h_on_done_epic_moves_to_review() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Done);
        t.epic_id = Some(EpicId(10));
        t
    }]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Done;
    app.board.epics = vec![epic];
    // Done epic → column 4
    app.selection_mut().set_column(4);
    app.selection_mut().set_row(4, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('H')));
    assert_eq!(app.board.epics[0].status, TaskStatus::Review);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist {
            id: EpicId(10),
            status: Some(TaskStatus::Review),
            ..
        })
    )));
}

#[test]
fn shift_e_key_starts_new_epic() {
    let mut app = make_app();
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('E'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::InputEpicTitle);
}

#[test]
fn space_key_on_epic_from_board_enters_epic_view() {
    let mut app = make_app_with_epic_selected();
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(matches!(
        app.board.view_mode,
        ViewMode::Epic {
            epic_id: EpicId(10),
            ..
        }
    ));
}

#[test]
fn e_key_in_epic_view_edits_epic() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('e'))));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Editor(crate::tui::commands::EditorCommand::PopOut(EditKind::EpicEdit(e))) if e.id == EpicId(10))
    );
}

#[test]
fn e_key_on_task_in_epic_view_edits_task_not_epic() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    let mut subtask = make_task(1, TaskStatus::Backlog);
    subtask.epic_id = Some(EpicId(10));
    app.board.tasks = vec![subtask];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));

    // Cursor on the subtask in the Backlog column (col 1, row 0)
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('e'))));
    assert_eq!(cmds.len(), 1, "expected exactly one command");
    assert!(
        matches!(&cmds[0], Command::Editor(crate::tui::commands::EditorCommand::PopOut(EditKind::TaskEdit(t))) if t.id == TaskId(1)),
        "expected PopOutEditor(TaskEdit(task 1), got {:?}",
        cmds
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn esc_in_epic_view_exits_to_board() {
    let mut app = App::new(vec![]);
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    app.handle_key(make_key(KeyCode::Esc));
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn shift_u_in_epic_view_toggles_auto_dispatch() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(42);
    epic.auto_dispatch = true;
    app.board.epics = vec![epic];

    // Enter epic view
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(42),
    )));

    // Press Shift+U — should return ToggleEpicAutoDispatch command with auto_dispatch = false
    let cmds = app.handle_key(make_key(KeyCode::Char('U')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::ToggleAutoDispatch {
            id: EpicId(42),
            auto_dispatch: false
        })
    )));

    // Also verify in-memory state was updated
    assert!(!app.board.epics[0].auto_dispatch);
}

#[test]
fn epic_title_esc_cancels() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.set_buffer("partial".to_string());
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
}

#[test]
fn epic_title_enter_with_text_advances_to_description() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.set_buffer("My Epic".to_string());
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputEpicDescription);
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.epic_draft.as_ref().unwrap().title, "My Epic");
}

#[test]
fn epic_title_enter_empty_cancels() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer.clear();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn epic_description_submit_completes_creation() {
    let mut app = make_app();
    app.input.mode = InputMode::InputEpicDescription;
    app.input.epic_draft = Some(EpicDraft {
        title: "My Epic".to_string(),
        description: String::new(),
        parent_epic_id: None,
    });
    let cmds = app.update(Message::Epic(
        crate::tui::messages::EpicMessage::SubmitDescription("Some description".to_string()),
    ));
    // Should immediately emit Insert command
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Insert(_))
    )));
    // Mode should be reset to Normal
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn epic_text_input_char_appends() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputEpicTitle;
    app.handle_key(make_key(KeyCode::Char('A')));
    app.handle_key(make_key(KeyCode::Char('b')));
    assert_eq!(app.input.buffer, "Ab");
}

#[test]
fn epic_text_input_backspace_removes() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.set_buffer("abc".to_string());
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "ab");
}

#[test]
fn epic_text_input_unrecognized_key_is_noop() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputEpicTitle;
    app.input.set_buffer("x".to_string());
    let cmds = app.handle_key(make_key(KeyCode::Tab));
    assert!(cmds.is_empty());
    assert_eq!(app.input.buffer, "x");
    assert_eq!(app.input.mode, InputMode::InputEpicTitle);
}

fn make_app_confirm_delete_epic() -> App {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1); // cursor on epic (same priority as task, sorts after by id)
    app.input.mode = InputMode::ConfirmDeleteEpic;
    app.status.message = Some("Delete epic \"Epic 10\" and subtasks? [y/n]".to_string());
    app
}

#[test]
fn confirm_delete_epic_enters_mode_with_title() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1); // cursor on epic (same priority as task, sorts after by id)
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ConfirmDelete,
    ));
    assert_eq!(app.input.mode, InputMode::ConfirmDeleteEpic);
    assert_eq!(
        app.status.message.as_deref(),
        Some("Delete epic \"Epic 10\" and subtasks? [y/n]")
    );
}

#[test]
fn confirm_delete_epic_y_deletes() {
    let mut app = make_app_confirm_delete_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status.message.is_none());
    assert!(app.board.epics.is_empty());
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Epic(crate::tui::commands::EpicCommand::Delete(id)) if *id == EpicId(10))));
}

#[test]
fn confirm_delete_epic_uppercase_y_deletes() {
    let mut app = make_app_confirm_delete_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('Y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.board.epics.is_empty());
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Epic(crate::tui::commands::EpicCommand::Delete(id)) if *id == EpicId(10))));
}

#[test]
fn confirm_delete_epic_other_key_cancels() {
    let mut app = make_app_confirm_delete_epic();
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status.message.is_none());
    assert_eq!(app.board.epics.len(), 1); // not deleted
    assert!(cmds.is_empty());
}

#[test]
fn confirm_delete_epic_no_epic_selected_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.selection_mut().set_column(1); // cursor on task, not epic
    app.input.mode = InputMode::ConfirmDeleteEpic;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('y'))));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty()); // no deletion happened
}

#[test]
fn space_key_on_epic_enters_epic_view() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Review;
    app.board.epics = vec![epic];

    // Even with subtasks that have tmux windows, space enters epic view
    let mut subtask = make_task(1, TaskStatus::Review);
    subtask.epic_id = Some(EpicId(10));
    subtask.tmux_window = Some("win-1".to_string());
    app.board.tasks = vec![subtask];

    app.selection_mut().set_column(3);
    app.selection_mut().set_row(3, 0);

    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(matches!(app.board.view_mode, ViewMode::Epic { epic_id, .. } if epic_id == EpicId(10)));
}

#[test]
fn shift_g_on_single_item_column_with_epic_is_noop() {
    // Only one selectable item (the epic itself) in the column: G jumping to
    // the "last" row lands on the same row it started on, and must never
    // enter epic view (that's still `space`'s job).
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.board.epics = vec![epic];

    let mut subtask = make_task(1, TaskStatus::Running);
    subtask.epic_id = Some(EpicId(10));
    subtask.sub_status = SubStatus::NeedsInput;
    subtask.tmux_window = Some("win-1".to_string());
    app.board.tasks = vec![subtask];

    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('G'))));
    assert!(
        cmds.is_empty(),
        "G on a single-item column emits no commands"
    );
    assert!(
        !matches!(app.board.view_mode, ViewMode::Epic { .. }),
        "G on an epic must not enter epic view"
    );
    assert_eq!(app.selection().row(2), 0);
}

#[test]
fn shift_g_jumps_to_last_row_in_column_with_an_epic() {
    // A column with an epic plus a plain task: G should jump to the last
    // selectable row (the same NavigateRowLast logic as `]`), not enter
    // epic view.
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Running),
        make_task(2, TaskStatus::Running),
    ]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.board.epics = vec![epic];

    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('G'))));
    assert!(cmds.is_empty());
    assert!(
        !matches!(app.board.view_mode, ViewMode::Epic { .. }),
        "G on an epic-containing column must not enter epic view"
    );
    assert_eq!(
        app.selection().row(2),
        2,
        "G should jump to the last row (epic + 2 tasks)"
    );
}

#[test]
fn column_items_sorted_by_sort_order() {
    let mut app = make_app();
    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.title = "First".to_string();
    t1.sort_order = Some(200);
    let mut t2 = make_task(2, TaskStatus::Backlog);
    t2.title = "Second".to_string();
    t2.sort_order = Some(100);
    app.board.tasks = vec![t1, t2];

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
    app.board.tasks = vec![t1, t2];

    let items = app.column_items_for_status(TaskStatus::Backlog);
    match &items[0] {
        ColumnItem::Task(t) => assert_eq!(t.title, "Low ID"),
        _ => panic!("expected task"),
    }
}

#[test]
fn done_column_sorts_by_completion_recency_via_sort_order() {
    let mut app = make_app();
    // sort_order values as the service layer would set them: negative
    // milliseconds, more negative = more recently completed.
    let mut older = make_task(1, TaskStatus::Done);
    older.title = "Completed first".to_string();
    older.sort_order = Some(-1_700_000_000_000);
    let mut newer = make_task(2, TaskStatus::Done);
    newer.title = "Completed second".to_string();
    newer.sort_order = Some(-1_700_000_100_000);
    app.board.tasks = vec![older, newer];

    let items = app.column_items_for_status(TaskStatus::Done);
    assert_eq!(items.len(), 2);
    match &items[0] {
        ColumnItem::Task(t) => assert_eq!(
            t.title, "Completed second",
            "the more recently completed task must render first"
        ),
        _ => panic!("expected task"),
    }
    match &items[1] {
        ColumnItem::Task(t) => assert_eq!(t.title, "Completed first"),
        _ => panic!("expected task"),
    }
}

#[test]
fn handle_key_normal_new_epic() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('E')));
    assert_eq!(*app.mode(), InputMode::InputEpicTitle);
}

#[test]
fn handle_key_normal_tab_is_noop_without_feed_epics() {
    // Tab from Board is a no-op when there are no feed epics.
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Tab));
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
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
fn v_toggles_epic_selection() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    // Epic is at row 0 in Backlog column (no standalone tasks)
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    app.handle_key(make_key(KeyCode::Char('v')));
    assert!(app.select.epics.contains(&EpicId(10)));
}

#[test]
fn v_on_epic_toggle_off() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    // Select
    app.handle_key(make_key(KeyCode::Char('v')));
    assert!(app.select.epics.contains(&EpicId(10)));

    // Deselect
    app.handle_key(make_key(KeyCode::Char('v')));
    assert!(!app.select.epics.contains(&EpicId(10)));
}

#[test]
fn v_on_empty_column_no_epics_is_noop() {
    let mut app = App::new(vec![]);
    // Navigate to Review column (empty)
    app.update(Message::NavigateColumn(2));
    app.handle_key(make_key(KeyCode::Char('v')));
    assert!(app.select.epics.is_empty());
    assert!(app.select.tasks.is_empty());
}

#[test]
fn select_all_column_includes_epics() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];

    app.update(Message::SelectAllColumn);
    assert!(app.select.tasks.contains(&TaskId(1)));
    assert!(app.select.epics.contains(&EpicId(10)));
}

#[test]
fn select_all_deselects_all_including_epics() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];

    // Select all
    app.update(Message::SelectAllColumn);
    assert_eq!(app.select.tasks.len(), 1);
    assert_eq!(app.select.epics.len(), 1);

    // Deselect all
    app.update(Message::SelectAllColumn);
    assert!(app.select.tasks.is_empty());
    assert!(app.select.epics.is_empty());
}

#[test]
fn select_all_column_with_only_epics() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];

    app.update(Message::SelectAllColumn);
    assert!(app.select.tasks.is_empty());
    assert_eq!(app.select.epics.len(), 2);
    assert!(app.select.epics.contains(&EpicId(10)));
    assert!(app.select.epics.contains(&EpicId(20)));
}

#[test]
fn esc_clears_epic_selection() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(10)),
    ));
    assert_eq!(app.select.epics.len(), 1);

    app.handle_key(make_key(KeyCode::Esc));
    assert!(app.select.epics.is_empty());
}

#[test]
fn x_key_with_epic_selection_shows_count_in_confirm() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(10)),
    ));
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(20)),
    ));

    app.handle_key(make_key(KeyCode::Char('x')));
    assert!(matches!(app.input.mode, InputMode::ConfirmArchive(None)));
    assert_eq!(
        app.status.message.as_deref(),
        Some("Archive 2 items? [y/n]")
    );
}

#[test]
fn shift_l_on_epic_moves_status_forward() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    // Cursor on Backlog column, row 0 (the epic)
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('L')));
    assert_eq!(app.board.epics[0].status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist {
            id: EpicId(10),
            status: Some(TaskStatus::Running),
            ..
        })
    )));
}

#[test]
fn render_selected_epic_shows_star_prefix() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(10)),
    ));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "* "),
        "Selected epic should show * prefix"
    );
    assert!(
        buffer_contains(&buf, "Epic 10"),
        "Epic title should be visible"
    );
}

#[test]
fn render_unselected_epic_no_star() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Epic 10"),
        "Epic title should be visible"
    );
    // The epic renders with "  " prefix (2 spaces), not "* "
    assert!(
        !buffer_contains(&buf, "* "),
        "Unselected epic should not show * prefix"
    );
}

#[test]
fn render_batch_hints_with_epic_selection() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(10)),
    ));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "1 selected"),
        "Should show selection count"
    );
    assert!(buffer_contains(&buf, "archive"), "Should show archive hint");
}

#[test]
fn render_column_header_checked_with_epics() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];

    // Select both the task and the epic
    app.update(Message::SelectAllColumn);
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "[x]"),
        "Checkbox should be checked when all items selected"
    );
}

#[test]
fn refresh_epics_prunes_stale_epic_selections() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(10)),
    ));
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(99)),
    )); // non-existent

    // Refresh with only epic 10
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Refresh(
        vec![make_epic(10)],
    )));
    assert!(app.select.epics.contains(&EpicId(10)));
    assert!(!app.select.epics.contains(&EpicId(99)));
}

#[test]
fn render_status_bar_confirm_delete_epic() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDeleteEpic;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Delete epic"),
        "ConfirmDeleteEpic should show 'Delete epic'"
    );
}

#[test]
fn render_status_bar_epic_title() {
    let mut app = make_app();
    app.input.mode = InputMode::InputEpicTitle;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Creating epic: enter title"),
        "InputEpicTitle should show 'Creating epic: enter title'"
    );
}

#[test]
fn render_status_bar_epic_description() {
    let mut app = make_app();
    app.input.mode = InputMode::InputEpicDescription;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Creating epic: opening $EDITOR for description"),
        "InputEpicDescription should show 'Creating epic: opening $EDITOR for description'"
    );
}

#[test]
fn render_input_form_epic_title_shows_new_epic() {
    let mut app = make_app();
    app.input.mode = InputMode::InputEpicTitle;
    app.input.set_buffer("My epic".to_string());
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "New Epic"),
        "block title 'New Epic' should be visible"
    );
    assert!(
        buffer_contains(&buf, "Title:"),
        "'Title:' label should be visible"
    );
    assert!(
        buffer_contains(&buf, "My epic"),
        "buffer text 'My epic' should be visible"
    );
}

#[test]
fn render_input_form_epic_description_shows_fields() {
    let mut app = make_app();
    app.input.mode = InputMode::InputEpicDescription;
    app.input.epic_draft = Some(EpicDraft {
        title: "Epic title".to_string(),
        ..Default::default()
    });
    app.input.set_buffer("Epic desc".to_string());
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "New Epic"),
        "block title 'New Epic' should be visible"
    );
    assert!(
        buffer_contains(&buf, "Epic title"),
        "completed title 'Epic title' should be visible"
    );
    assert!(
        buffer_contains(&buf, "Description:"),
        "'Description:' label should be visible"
    );
}

#[test]
fn render_epic_banner_shows_title() {
    let mut app = make_app();
    let mut epic = make_epic(10);
    epic.title = "Auth Refactor".to_string();
    app.board.epics = vec![epic];
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Auth Refactor"),
        "epic banner should show the epic title 'Auth Refactor'"
    );
}

#[test]
fn render_epic_banner_not_shown_in_board_view() {
    let mut app = make_app();
    let epic = make_epic(10);
    app.board.epics = vec![epic];
    // Stay in default Board view — do not switch to ViewMode::Epic
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        !buffer_contains(&buf, "Esc to return"),
        "epic banner should not be shown in Board view"
    );
}

#[test]
fn render_detail_task_with_epic_reference() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.epic_id = Some(EpicId(10));
    let mut epic = make_epic(10);
    epic.title = "Auth Epic".to_string();
    let mut app = App::new(vec![task]);
    app.board.epics = vec![epic];
    // Switch to Epic view so the subtask is visible (Board view hides epic subtasks)
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    // This test will be updated in Task 6 to use the overlay.
    let _buf = render_to_buffer(&mut app, 160, 30);
}

#[test]
fn render_detail_epic_shows_title_and_id() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.title = "Platform Migration".to_string();
    app.board.epics = vec![epic];
    // Epic is the only item in Backlog column (no standalone tasks)
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    // This test will be updated in Task 6 to use the overlay.
    let _buf = render_to_buffer(&mut app, 120, 30);
}

#[test]
fn render_detail_epic_with_plan_shows_path() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.plan_path = Some("docs/plans/migration.md".to_string());
    app.board.epics = vec![epic];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    // This test will be updated in Task 6 to use the overlay.
    let _buf = render_to_buffer(&mut app, 120, 30);
}

#[test]
fn render_detail_epic_shows_subtask_list() {
    let mut app = App::new(vec![]);
    let epic = make_epic(10);
    app.board.epics = vec![epic];

    let mut t1 = make_task(101, TaskStatus::Done);
    t1.title = "Subtask Alpha".to_string();
    t1.epic_id = Some(EpicId(10));
    let mut t2 = make_task(102, TaskStatus::Running);
    t2.title = "Subtask Beta".to_string();
    t2.epic_id = Some(EpicId(10));
    app.board.tasks = vec![t1, t2];

    // Epic is in Backlog; subtasks are in other columns so won't appear as
    // standalone items in column 1 (Backlog). The epic itself is the first item.
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    // This test will be updated in Task 6 to use the overlay.
    let _buf = render_to_buffer(&mut app, 120, 30);
}

#[test]
fn render_detail_epic_subtask_conflict_shows_warning() {
    let mut app = App::new(vec![]);
    let epic = make_epic(10);
    app.board.epics = vec![epic];

    let mut t1 = make_task(201, TaskStatus::Running);
    t1.title = "Conflicted Task".to_string();
    t1.epic_id = Some(EpicId(10));
    t1.sub_status = SubStatus::Conflict;
    app.board.tasks = vec![t1];

    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    // This test will be updated in Task 6 to use the overlay.
    let _buf = render_to_buffer(&mut app, 120, 30);
}

#[test]
fn render_tab_bar_epic_mode_has_no_tasks_label() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.title = "Platform Work".to_string();
    app.board.epics = vec![epic];
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        !buffer_contains(&buf, "Tasks"),
        "tab bar should not show a 'Tasks' tab label in any mode"
    );
}

#[test]
fn epic_card_title_truncated_in_narrow_terminal() {
    let mut epic = make_epic(1);
    epic.title = "This is a very long epic title that should be truncated to fit".to_string();
    let mut app = App::new(vec![]);
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Refresh(
        vec![epic],
    )));

    let buf = render_to_buffer(&mut app, 80, 10);
    assert!(
        !buffer_contains(
            &buf,
            "This is a very long epic title that should be truncated to fit"
        ),
        "full epic title should be truncated in narrow terminal"
    );
}

#[test]
fn handle_key_normal_esc_in_epic_view_exits() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));
    assert!(matches!(app.board.view_mode, ViewMode::Epic { .. }));

    app.handle_key(make_key(KeyCode::Esc));
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn handle_key_normal_q_in_epic_view_exits() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));

    app.handle_key(make_key(KeyCode::Char('q')));
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn handle_key_e_in_tag_input_does_not_set_a_tag() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    app.handle_key(make_key(KeyCode::Char('e')));
    assert_eq!(app.input.task_draft.as_ref().unwrap().tag, None);
}

#[test]
fn handle_key_normal_shift_l_on_epic_moves_status() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('L')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist { .. })
    )));
}

#[test]
fn handle_key_normal_shift_h_on_epic_moves_backward() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Running;
    app.board.epics = vec![epic];
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('H')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Persist { .. })
    )));
}

/// InputEpicTitle mode routes to the text input handler.
#[test]
fn handle_key_input_epic_title_routes_to_text_input() {
    let mut app = make_app();
    app.input.mode = InputMode::InputEpicTitle;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// InputEpicDescription mode routes to the text input handler.
#[test]
fn handle_key_input_epic_description_routes_to_text_input() {
    let mut app = make_app();
    app.input.mode = InputMode::InputEpicDescription;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// ConfirmDeleteEpic mode routes correctly.
#[test]
fn handle_key_confirm_delete_epic_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDeleteEpic;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// Normal mode on Epic view routes to the board handler (not review/security).
#[test]
fn handle_key_normal_epic_view_routes_correctly() {
    let mut app = make_app();
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(1),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    // 'q' in epic view exits to board (doesn't quit)
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('q'))));
    assert!(cmds.is_empty());
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn epic_view_header_shows_auto_dispatch_indicator() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(1);
    epic.auto_dispatch = true;
    app.board.epics = vec![epic];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(1),
    )));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "auto dispatch [U]"),
        "Expected 'auto dispatch [U]' in header"
    );
}

#[test]
fn epic_view_header_shows_manual_dispatch_indicator() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(1);
    epic.auto_dispatch = false;
    app.board.epics = vec![epic];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(1),
    )));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "manual dispatch [U]"),
        "Expected 'manual dispatch [U]' in header"
    );
}

#[test]
fn exit_sub_epic_returns_to_parent_epic() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(1), make_epic(2)];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(1),
    )));
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(2),
    )));

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Exit));

    match &app.board.view_mode {
        ViewMode::Epic { epic_id, .. } => {
            assert_eq!(*epic_id, EpicId(1), "should return to parent epic 1");
        }
        _ => panic!("Expected ViewMode::Epic after exiting sub-epic"),
    }
}

#[test]
fn exit_from_root_epic_returns_to_board() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(1)];
    app.selection_mut().set_column(3);
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(1),
    )));
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Exit));

    match &app.board.view_mode {
        ViewMode::Board(sel) => {
            assert_eq!(sel.column(), 3, "board column should be restored");
        }
        _ => panic!("Expected ViewMode::Board"),
    }
}

#[test]
fn board_view_excludes_sub_epics() {
    let mut app = App::new(vec![]);
    let mut sub = make_epic(20);
    sub.parent_epic_id = Some(EpicId(10));
    app.board.epics = vec![make_epic(10), sub];

    let items = app.column_items_for_status(TaskStatus::Backlog);
    // Only root epic (id=10) should appear; sub-epic (id=20) must not
    let epic_ids: Vec<i64> = items
        .iter()
        .filter_map(|i| {
            if let ColumnItem::Epic(e) = i {
                Some(e.id.0)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(epic_ids, vec![10], "only root epic should appear on board");
}

#[test]
fn epic_view_includes_sub_epics_as_column_items() {
    let mut app = App::new(vec![]);
    let mut sub = make_epic(20);
    sub.parent_epic_id = Some(EpicId(10));
    app.board.epics = vec![make_epic(10), sub];

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));

    let items = app.column_items_for_status(TaskStatus::Backlog);
    // sub-epic (id=20) should appear as an Epic column item
    let epic_ids: Vec<i64> = items
        .iter()
        .filter_map(|i| {
            if let ColumnItem::Epic(e) = i {
                Some(e.id.0)
            } else {
                None
            }
        })
        .collect();
    assert!(
        epic_ids.contains(&20),
        "sub-epic should appear inside parent epic view"
    );
}

#[test]
fn epic_view_breadcrumb_shows_parent_and_child_title() {
    let mut app = App::new(vec![]);
    let parent_epic = make_epic_with_title(1, "Root Epic");
    let child_epic = make_epic_with_title(2, "Child Epic");
    app.board.epics = vec![parent_epic.clone(), child_epic.clone()];

    // Nested: viewing child epic, parent is another epic view
    app.board.view_mode = ViewMode::Epic {
        epic_id: child_epic.id,
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Epic {
            epic_id: parent_epic.id,
            selection: BoardSelection::new_for_epic(),
            parent: Box::new(ViewMode::Board(BoardSelection::new())),
        }),
    };

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Root Epic"),
        "breadcrumb should show parent epic title"
    );
    assert!(
        buffer_contains(&buf, "Child Epic"),
        "breadcrumb should show current epic title"
    );
    // The separator between parent and child
    assert!(
        buffer_contains(&buf, "›"),
        "breadcrumb should show › separator between parent and child"
    );
}

#[test]
fn epic_view_no_breadcrumb_when_parent_is_board() {
    let mut app = App::new(vec![]);
    let epic = make_epic_with_title(1, "Only Epic");
    app.board.epics = vec![epic.clone()];
    app.board.view_mode = ViewMode::Epic {
        epic_id: epic.id,
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Only Epic"),
        "title should show current epic title"
    );
}

#[test]
fn create_epic_in_epic_view_inherits_parent() {
    let mut app = App::new(vec![]);
    let parent_id = EpicId(42);
    app.board.view_mode = ViewMode::Epic {
        epic_id: parent_id,
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };

    // Enter the epic creation flow
    app.update(Message::Epic(crate::tui::messages::EpicMessage::StartNew));
    assert_eq!(app.input.mode, InputMode::InputEpicTitle);

    // The draft should already know about the parent
    let draft_parent = app.input.epic_draft.as_ref().and_then(|d| d.parent_epic_id);
    assert_eq!(
        draft_parent,
        Some(parent_id),
        "epic_draft.parent_epic_id should be set to current epic's id"
    );

    // Submit title then description — description submit now completes creation
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::SubmitTitle("Sub Epic".to_string()),
    ));
    let cmds = app.update(Message::Epic(
        crate::tui::messages::EpicMessage::SubmitDescription("desc".to_string()),
    ));

    let draft = cmds
        .iter()
        .find_map(|c| {
            if let Command::Epic(crate::tui::commands::EpicCommand::Insert(d)) = c {
                Some(d)
            } else {
                None
            }
        })
        .expect("expected Command::InsertEpic");

    assert_eq!(
        draft.parent_epic_id,
        Some(parent_id),
        "InsertEpic draft must carry parent_epic_id"
    );
}

#[test]
fn breadcrumb_shows_three_levels() {
    let mut app = App::new(vec![]);
    let grandparent = make_epic_with_title(1, "Grandparent");
    let parent = make_epic_with_title(2, "ParentEpic");
    let child = make_epic_with_title(3, "ChildEpic");
    app.board.epics = vec![grandparent, parent, child];

    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(3),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Epic {
            epic_id: EpicId(2),
            selection: BoardSelection::new_for_epic(),
            parent: Box::new(ViewMode::Epic {
                epic_id: EpicId(1),
                selection: BoardSelection::new_for_epic(),
                parent: Box::new(ViewMode::Board(BoardSelection::new())),
            }),
        }),
    };

    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "Grandparent"),
        "breadcrumb should show grandparent title"
    );
    assert!(
        buffer_contains(&buf, "ParentEpic"),
        "breadcrumb should show parent title"
    );
    assert!(
        buffer_contains(&buf, "ChildEpic"),
        "breadcrumb should show child title"
    );
}

#[test]
fn test_epic_anchor_preserved_on_refresh() {
    let tasks = vec![make_task(1, TaskStatus::Backlog)];
    let epics = vec![make_epic(1)];
    let mut app = App::new(tasks.clone());
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Refresh(
        epics.clone(),
    )));

    let items = app.column_items_for_status(TaskStatus::Backlog);
    let epic_row = items
        .iter()
        .position(|i| matches!(i, ColumnItem::Epic(_)))
        .expect("epic should be in Backlog column");
    for _ in 0..epic_row {
        app.update(Message::NavigateRow(1));
    }
    assert!(matches!(
        app.column_items_for_status(TaskStatus::Backlog)[app.selection().row(1)],
        ColumnItem::Epic(_)
    ));

    // Refresh same data
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        tasks,
    )));
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Refresh(
        epics,
    )));

    // Still on the epic
    assert!(matches!(
        app.column_items_for_status(TaskStatus::Backlog)[app.selection().row(1)],
        ColumnItem::Epic(_)
    ));
}

#[test]
fn epic_view_navigation_does_not_enter_projects_or_archive() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));
    assert!(matches!(app.board.view_mode, ViewMode::Epic { .. }));

    // Starts at Backlog (column 1)
    assert_eq!(app.selected_column(), 1);

    // Navigate left past Backlog — should not enter Projects (col 0)
    app.update(Message::NavigateColumn(-1));
    assert_eq!(
        app.selected_column(),
        1,
        "should not enter Projects (col 0) from epic view"
    );

    // Navigate right to Done (col 4)
    for _ in 0..3 {
        app.update(Message::NavigateColumn(1));
    }
    assert_eq!(app.selected_column(), 4);

    // Navigate right past Done — should not enter Archive (col 5)
    app.update(Message::NavigateColumn(1));
    assert_eq!(
        app.selected_column(),
        4,
        "should not enter Archive (col 5) from epic view"
    );
}

#[test]
fn test_selection_survives_flatten_toggle() {
    // Use task IDs > 1 so they sort after Epic(1) in the column.
    // Column order: [Task(1), Epic(1), Task(2)] — tasks inserted before epics,
    // stable sort keeps Task(1) before Epic(1) when both have key (5,1,1).
    // Navigate +2 to reach Task(2) at row 2.
    let tasks = vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ];
    let epics = vec![make_epic(1)];
    let mut app = App::new(tasks.clone());
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Refresh(
        epics.clone(),
    )));

    app.update(Message::NavigateRow(1)); // row 1 — Epic(1)
    app.update(Message::NavigateRow(1)); // row 2 — Task(2)
    let items = app.column_items_for_status(TaskStatus::Backlog);
    let pre_id: TaskId = match &items[app.selection().row(0)] {
        ColumnItem::Task(t) => t.id,
        _ => panic!("expected task at cursor"),
    };

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleFlattened,
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleFlattened,
    ));

    let items = app.column_items_for_status(TaskStatus::Backlog);
    let post_id: TaskId = match &items[app.selection().row(0)] {
        ColumnItem::Task(t) => t.id,
        _ => panic!("expected task at cursor"),
    };
    assert_eq!(pre_id, post_id);
}

#[test]
fn reparent_epic_message_and_command_variants_compile() {
    use crate::tui::commands::EpicCommand;
    use crate::tui::messages::EpicMessage;
    use crate::tui::types::TreeNav;

    let _msgs = [
        EpicMessage::StartReparent(EpicId(1)),
        EpicMessage::ReparentNavigate(TreeNav::Down),
        EpicMessage::ReparentConfirm,
        EpicMessage::ReparentExecute,
        EpicMessage::ReparentCancel,
    ];
    let _cmd = EpicCommand::Reparent {
        id: EpicId(1),
        new_parent: Some(EpicId(2)),
    };
    let _cmd_root = EpicCommand::Reparent {
        id: EpicId(1),
        new_parent: None,
    };
}

// ---------------------------------------------------------------------------
// Feed epic manual trigger — message handling
// ---------------------------------------------------------------------------

#[test]
fn trigger_epic_feed_sets_status_and_returns_command() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.feed_command = Some("echo '[]'".to_string());
    app.board.epics = vec![epic];

    let cmds = app.update(Message::Feed(
        crate::tui::messages::FeedMessage::TriggerEpic(EpicId(10)),
    ));

    assert!(
        app.status_message()
            .map(|s| s.contains("Epic 10"))
            .unwrap_or(false),
        "status should mention epic title"
    );
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Feed(crate::tui::commands::FeedCommand::TriggerEpic {
                epic_id,
                ..
            }) if *epic_id == EpicId(10)
        )),
        "should return TriggerEpicFeed command"
    );
}

#[test]
fn trigger_epic_feed_no_feed_command_sets_status_no_command() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)]; // no feed_command

    let cmds = app.update(Message::Feed(
        crate::tui::messages::FeedMessage::TriggerEpic(EpicId(10)),
    ));

    assert!(
        app.status_message().is_some(),
        "should show a status message"
    );
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Feed(crate::tui::commands::FeedCommand::TriggerEpic { .. })
        )),
        "should not return TriggerEpicFeed command when no feed_command"
    );
}

#[test]
fn feed_refreshed_sets_status_and_returns_refresh_from_db() {
    let mut app = App::new(vec![]);

    let cmds = app.update(Message::Feed(
        crate::tui::messages::FeedMessage::Refreshed {
            epic_title: "My Feed Epic".to_string(),
            count: 5,
            wrote_stderr: false,
        },
    ));

    let status = app.status_message().unwrap_or("");
    assert!(
        status.contains("My Feed Epic"),
        "status should mention epic title, got: {status}"
    );
    assert!(
        status.contains("5"),
        "status should mention task count, got: {status}"
    );
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::RefreshFromDb)
        )),
        "should return RefreshFromDb command"
    );
}

#[test]
fn feed_refreshed_zero_items_still_succeeds() {
    let mut app = App::new(vec![]);

    let cmds = app.update(Message::Feed(
        crate::tui::messages::FeedMessage::Refreshed {
            epic_title: "Empty Feed".to_string(),
            count: 0,
            wrote_stderr: false,
        },
    ));

    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::RefreshFromDb)
        )),
        "zero-item refresh should still return RefreshFromDb"
    );
}

#[test]
fn feed_refreshed_zero_items_with_stderr_hints_at_the_log() {
    let mut app = App::new(vec![]);

    let cmds = app.update(Message::Feed(
        crate::tui::messages::FeedMessage::Refreshed {
            epic_title: "PR Reviews".to_string(),
            count: 0,
            wrote_stderr: true,
        },
    ));

    let status = app.status_message().unwrap_or("");
    assert!(
        status.contains("command wrote to stderr"),
        "a 0-item sync whose command wrote to stderr must point at the log, got: {status}"
    );
    assert!(
        status.contains("app.log"),
        "the hint must name app.log so the user knows where to look, got: {status}"
    );
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::RefreshFromDb)
        )),
        "the sync still succeeded, so it must still refresh"
    );
}

#[test]
fn feed_refreshed_zero_items_without_stderr_has_no_hint() {
    let mut app = App::new(vec![]);

    app.update(Message::Feed(
        crate::tui::messages::FeedMessage::Refreshed {
            epic_title: "Empty Feed".to_string(),
            count: 0,
            wrote_stderr: false,
        },
    ));

    let status = app.status_message().unwrap_or("");
    assert!(
        !status.contains("stderr"),
        "a genuinely empty feed is not an error and must not be flagged, got: {status}"
    );
}

// Gated on count == 0 deliberately: a script that writes harmless progress
// chatter to stderr must not nag on every successful refresh.
#[test]
fn feed_refreshed_with_items_and_stderr_has_no_hint() {
    let mut app = App::new(vec![]);

    app.update(Message::Feed(
        crate::tui::messages::FeedMessage::Refreshed {
            epic_title: "Chatty Feed".to_string(),
            count: 7,
            wrote_stderr: true,
        },
    ));

    let status = app.status_message().unwrap_or("");
    assert!(
        !status.contains("stderr"),
        "items synced, so stderr is not worth interrupting for, got: {status}"
    );
}

#[test]
fn feed_failed_sets_status_no_refresh() {
    let mut app = App::new(vec![]);

    let cmds = app.update(Message::Feed(crate::tui::messages::FeedMessage::Failed {
        epic_title: "Bad Feed".to_string(),
        error: "exit code 1".to_string(),
    }));

    let status = app.status_message().unwrap_or("");
    assert!(
        status.contains("Bad Feed"),
        "status should mention epic title, got: {status}"
    );
    assert!(
        status.contains("exit code 1"),
        "status should mention the error, got: {status}"
    );
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::RefreshFromDb)
        )),
        "failed feed should NOT return RefreshFromDb"
    );
}

#[test]
fn epic_action_hints_shows_refresh_for_feed_epic() {
    let mut epic = make_epic(1);
    epic.feed_command = Some("echo '[]'".to_string());
    let hints = ui::epic_action_hints(&epic, ratatui::style::Color::Rgb(122, 162, 247));
    let keys = hint_keys(&hints);
    assert!(
        keys.contains(&"[r]"),
        "feed epic should show [r] refresh hint"
    );
}

#[test]
fn epic_action_hints_no_refresh_for_non_feed_epic() {
    let epic = make_epic(1); // no feed_command
    let hints = ui::epic_action_hints(&epic, ratatui::style::Color::Rgb(122, 162, 247));
    let keys = hint_keys(&hints);
    assert!(
        !keys.contains(&"[r]"),
        "non-feed epic should NOT show [r] refresh hint"
    );
}

#[test]
fn flat_view_inserts_epic_header_before_group() {
    let mut app = App::new(vec![]);
    let epic = make_epic(10);
    app.board.epics = vec![epic];
    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(EpicId(10));
    let mut t2 = make_task(2, TaskStatus::Running);
    t2.epic_id = Some(EpicId(10));
    app.board.tasks = vec![t1, t2];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Running);
    let selectable: Vec<_> = items.iter().filter(|i| i.is_selectable()).collect();
    assert_eq!(selectable.len(), 2, "2 tasks");
    let headers: Vec<_> = items
        .iter()
        .filter(|i| matches!(i, ColumnItem::EpicHeader(_)))
        .collect();
    assert_eq!(headers.len(), 1, "one EpicHeader");
    assert!(
        matches!(headers[0], ColumnItem::EpicHeader(e) if e.id == EpicId(10)),
        "header must be for epic 10"
    );
}

#[test]
fn flat_view_header_sorts_before_its_tasks() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.sort_order = Some(50);
    app.board.epics = vec![epic];
    let mut t = make_task(1, TaskStatus::Running);
    t.epic_id = Some(EpicId(10));
    t.sort_order = Some(100);
    app.board.tasks = vec![t];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Running);
    let header_pos = items
        .iter()
        .position(|i| matches!(i, ColumnItem::EpicHeader(_)));
    let task_pos = items.iter().position(|i| matches!(i, ColumnItem::Task(_)));
    assert!(header_pos.is_some() && task_pos.is_some());
    assert!(
        header_pos.unwrap() < task_pos.unwrap(),
        "header precedes task"
    );
}

#[test]
fn flat_view_standalone_task_interleaves_by_sort_order() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.sort_order = Some(200);
    app.board.epics = vec![epic];

    let mut standalone = make_task(1, TaskStatus::Running);
    standalone.sort_order = Some(100); // orphan tasks sort last (epic_sk = i64::MAX)

    let mut subtask = make_task(2, TaskStatus::Running);
    subtask.epic_id = Some(EpicId(10));
    subtask.sort_order = Some(300);

    app.board.tasks = vec![standalone, subtask];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Running);
    // epic group comes first, orphan (no epic in board) sorts last with epic_sk = i64::MAX.
    // An OrphanSeparator is emitted between the epic group and the orphan task.
    assert!(matches!(items[0], ColumnItem::SubstatusLabel(_)));
    assert!(matches!(items[1], ColumnItem::EpicHeader(_)));
    assert!(matches!(items[2], ColumnItem::Task(t) if t.id == TaskId(2)));
    assert!(matches!(items[3], ColumnItem::OrphanSeparator));
    assert!(matches!(items[4], ColumnItem::Task(t) if t.id == TaskId(1)));
}

#[test]
fn flat_view_two_epics_get_two_headers() {
    let mut app = App::new(vec![]);
    let mut epic_a = make_epic(10);
    epic_a.sort_order = Some(10);
    let mut epic_b = make_epic(20);
    epic_b.sort_order = Some(20);
    app.board.epics = vec![epic_a, epic_b];

    let mut ta = make_task(1, TaskStatus::Running);
    ta.epic_id = Some(EpicId(10));
    let mut tb = make_task(2, TaskStatus::Running);
    tb.epic_id = Some(EpicId(20));
    app.board.tasks = vec![ta, tb];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Running);
    let headers: Vec<_> = items
        .iter()
        .filter(|i| matches!(i, ColumnItem::EpicHeader(_)))
        .collect();
    assert_eq!(headers.len(), 2, "one header per epic");
}

#[test]
fn flat_view_no_header_when_epic_has_no_tasks_in_column() {
    let mut app = App::new(vec![]);
    // Epic 10 has a task in Running, not Backlog
    app.board.epics = vec![make_epic(10)];
    let mut t = make_task(1, TaskStatus::Running);
    t.epic_id = Some(EpicId(10));
    app.board.tasks = vec![t];
    app.board.flattened = true;

    let backlog = app.column_items_for_status(TaskStatus::Backlog);
    assert!(
        backlog
            .iter()
            .all(|i| !matches!(i, ColumnItem::EpicHeader(_))),
        "no header in Backlog when epic has no Backlog tasks"
    );
}

#[test]
fn flat_view_orphan_task_treated_as_standalone() {
    let mut app = App::new(vec![]);
    // No epics in board.epics, but a task references epic id 99
    let mut orphan = make_task(1, TaskStatus::Running);
    orphan.epic_id = Some(EpicId(99));
    app.board.tasks = vec![orphan];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Running);
    let selectable: Vec<_> = items.iter().filter(|i| i.is_selectable()).collect();
    assert_eq!(selectable.len(), 1, "orphan renders without a header");
    assert!(matches!(selectable[0], ColumnItem::Task(_)));
}

#[test]
fn flat_view_tie_break_by_epic_id_when_sort_orders_equal() {
    let mut app = App::new(vec![]);
    let mut epic_a = make_epic(10);
    epic_a.sort_order = Some(50);
    let mut epic_b = make_epic(20);
    epic_b.sort_order = Some(50); // same sort_order, tie-broken by id
    app.board.epics = vec![epic_a, epic_b];

    let mut ta = make_task(1, TaskStatus::Running);
    ta.epic_id = Some(EpicId(10));
    let mut tb = make_task(2, TaskStatus::Running);
    tb.epic_id = Some(EpicId(20));
    app.board.tasks = vec![ta, tb];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Running);
    let headers: Vec<EpicId> = items
        .iter()
        .filter_map(|i| {
            if let ColumnItem::EpicHeader(e) = i {
                Some(e.id)
            } else {
                None
            }
        })
        .collect();
    // Headers are emitted inline before their tasks; tie-broken by epic id.
    assert_eq!(headers, vec![EpicId(10), EpicId(20)]);
}

#[test]
fn non_flat_mode_has_no_epic_headers() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    let mut t = make_task(1, TaskStatus::Backlog);
    t.epic_id = Some(EpicId(10));
    app.board.tasks = vec![t];
    // flattened = false (default)

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(
        items
            .iter()
            .all(|i| !matches!(i, ColumnItem::EpicHeader(_))),
        "non-flat mode must never emit EpicHeader"
    );
}

#[test]
fn flat_view_selected_column_item_skips_headers() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    let mut t = make_task(1, TaskStatus::Running);
    t.epic_id = Some(EpicId(10));
    app.board.tasks = vec![t];
    app.board.flattened = true;

    // Items are [SubstatusLabel, EpicHeader(10), Task(1)]. selected_row=0 should give Task(1).
    app.selection_mut().set_column(2); // Running = nav col 2
    app.selection_mut().set_row(2, 0);

    let item = app.selected_column_item();
    assert!(
        matches!(item, Some(ColumnItem::Task(t)) if t.id == TaskId(1)),
        "row 0 should resolve to Task(1), not the header"
    );
}

#[test]
fn flat_view_review_substatus_label_precedes_epic_header() {
    // AwaitingReview has column_priority=5; Approved has column_priority=6.
    // Lower priority number = more urgent = sorts first.
    // SubstatusLabel must appear BEFORE the EpicHeader in each group.
    use crate::models::{EpicId, SubStatus};
    let mut app = App::new(vec![]);
    let epic = make_epic_with_title(10, "My Epic");
    app.board.epics = vec![epic];

    let mut t1 = make_task(1, TaskStatus::Review);
    t1.epic_id = Some(EpicId(10));
    t1.sub_status = SubStatus::AwaitingReview; // priority 5 — sorts first

    let mut t2 = make_task(2, TaskStatus::Review);
    t2.epic_id = Some(EpicId(10));
    t2.sub_status = SubStatus::Approved; // priority 6 — sorts after

    app.board.tasks = vec![t1, t2];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Review);
    // Expected: SubstatusLabel, EpicHeader, Task(1), SubstatusLabel, EpicHeader, Task(2)
    assert_eq!(items.len(), 6, "expected 6 items, got {}", items.len());
    assert!(
        matches!(items[0], ColumnItem::SubstatusLabel(_)),
        "items[0] must be SubstatusLabel"
    );
    assert!(
        matches!(items[1], ColumnItem::EpicHeader(_)),
        "items[1] must be EpicHeader"
    );
    assert!(
        matches!(items[2], ColumnItem::Task(_)),
        "items[2] must be Task"
    );
    assert!(
        matches!(items[3], ColumnItem::SubstatusLabel(_)),
        "items[3] must be SubstatusLabel"
    );
    assert!(
        matches!(items[4], ColumnItem::EpicHeader(_)),
        "items[4] must be EpicHeader"
    );
    assert!(
        matches!(items[5], ColumnItem::Task(_)),
        "items[5] must be Task"
    );
}

#[test]
fn flat_view_epic_repeated_across_substatus_groups() {
    // Same epic has tasks in two substatus groups.
    // EpicHeader for that epic must appear once per group (twice total).
    use crate::models::{EpicId, SubStatus};
    let mut app = App::new(vec![]);
    let epic = make_epic_with_title(10, "Shared Epic");
    app.board.epics = vec![epic];

    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(EpicId(10));
    t1.sub_status = SubStatus::NeedsInput; // priority 3

    let mut t2 = make_task(2, TaskStatus::Running);
    t2.epic_id = Some(EpicId(10));
    t2.sub_status = SubStatus::Active; // priority 5

    app.board.tasks = vec![t1, t2];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Running);
    let epic_header_count = items
        .iter()
        .filter(|i| matches!(i, ColumnItem::EpicHeader(_)))
        .count();
    assert_eq!(
        epic_header_count, 2,
        "EpicHeader must appear once per substatus group; got {epic_header_count}"
    );
}

#[test]
fn flat_view_backlog_no_substatus_labels() {
    // Backlog tasks don't have meaningful substatus groups — no SubstatusLabel expected.
    use crate::models::EpicId;
    let mut app = App::new(vec![]);
    let epic = make_epic_with_title(10, "Epic");
    app.board.epics = vec![epic];

    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.epic_id = Some(EpicId(10));

    app.board.tasks = vec![t1];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(
        !items
            .iter()
            .any(|i| matches!(i, ColumnItem::SubstatusLabel(_))),
        "Backlog column must not contain SubstatusLabel items"
    );
}

#[test]
fn shift_r_in_epic_view_toggles_group_by_repo() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(42);
    epic.group_by_repo = false;
    epic.feed_command = Some("echo '[]'".to_string());
    app.board.epics = vec![epic];

    // Enter epic view
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(42),
    )));

    // Press Shift+R — should return ToggleGroupByRepo command with group_by_repo = true
    let cmds = app.handle_key(make_key(KeyCode::Char('R')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::ToggleGroupByRepo {
            id: EpicId(42),
            group_by_repo: true
        })
    )));

    // Also verify in-memory state was updated
    assert!(app.board.epics[0].group_by_repo);
}

#[test]
fn shift_r_outside_epic_view_is_noop() {
    let mut app = App::new(vec![]);
    // Default view mode is not ViewMode::Epic
    let cmds = app.handle_key(make_key(KeyCode::Char('R')));
    assert!(
        cmds.is_empty(),
        "R outside epic view should produce no commands, got {cmds:?}"
    );
}

#[test]
fn epic_view_header_shows_group_by_repo_on_for_feed_epic() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(1);
    epic.feed_command = Some("echo '[]'".to_string());
    epic.group_by_repo = true;
    app.board.epics = vec![epic];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(1),
    )));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "group:on [R]"),
        "Expected 'group:on [R]' in header for feed epic with group_by_repo=true"
    );
}

#[test]
fn epic_view_header_shows_group_by_repo_off_for_feed_epic() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(1);
    epic.feed_command = Some("echo '[]'".to_string());
    epic.group_by_repo = false;
    app.board.epics = vec![epic];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(1),
    )));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "group:off [R]"),
        "Expected 'group:off [R]' in header for feed epic with group_by_repo=false"
    );
}

#[test]
fn epic_view_header_shows_group_indicator_for_non_feed_epic() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(1);
    epic.feed_command = None;
    epic.group_by_repo = false;
    app.board.epics = vec![epic];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(1),
    )));

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "group:off [R]"),
        "Expected 'group:off [R]' indicator in header for non-feed epic with group_by_repo=false"
    );
}

#[test]
fn flat_view_emits_orphan_separator_between_epic_and_orphan_tasks() {
    use crate::models::EpicId;
    use crate::tui::tests::helpers::make_epic_with_title;

    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(10, "Epic A")];
    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(EpicId(10));
    let mut t2 = make_task(2, TaskStatus::Running);
    t2.epic_id = None; // orphan
    app.board.tasks = vec![t1, t2];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Running);

    // Expected order: SubstatusLabel, EpicHeader, Task(epic), OrphanSeparator, Task(orphan)
    let header_pos = items
        .iter()
        .position(|i| matches!(i, ColumnItem::EpicHeader(_)));
    let epic_task_pos = items
        .iter()
        .position(|i| matches!(i, ColumnItem::Task(t) if t.id == TaskId(1)));
    let sep_pos = items
        .iter()
        .position(|i| matches!(i, ColumnItem::OrphanSeparator));
    let orphan_pos = items
        .iter()
        .position(|i| matches!(i, ColumnItem::Task(t) if t.id == TaskId(2)));
    assert!(header_pos.is_some(), "EpicHeader must be present");
    assert!(epic_task_pos.is_some(), "epic task must be present");
    assert!(sep_pos.is_some(), "OrphanSeparator must be present");
    assert!(orphan_pos.is_some(), "orphan task must be present");
    let h = header_pos.unwrap();
    let et = epic_task_pos.unwrap();
    let s = sep_pos.unwrap();
    let o = orphan_pos.unwrap();
    assert!(
        h < et && et < s && s < o,
        "order: header → epic-task → separator → orphan"
    );
}

#[test]
fn flat_view_no_orphan_separator_when_only_orphan_tasks() {
    let mut app = App::new(vec![]);
    let t1 = make_task(1, TaskStatus::Backlog);
    let t2 = make_task(2, TaskStatus::Backlog);
    // both tasks have epic_id = None (make_task default)
    app.board.tasks = vec![t1, t2];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Backlog);

    assert!(
        !items
            .iter()
            .any(|i| matches!(i, ColumnItem::OrphanSeparator)),
        "no separator when no epic tasks precede orphans"
    );
}

#[test]
fn flat_view_orphan_separator_resets_on_substatus_boundary() {
    // Running column: epic task (NeedsInput priority), orphan (NeedsInput),
    // then orphan (Active priority). OrphanSeparator appears once at the
    // NeedsInput epic→orphan transition; the Active band starts fresh
    // (current_epic_id reset to None), so no second separator.
    use crate::models::{EpicId, SubStatus};
    use crate::tui::tests::helpers::make_epic_with_title;

    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(10, "Epic A")];

    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(EpicId(10));
    t1.sub_status = SubStatus::NeedsInput;

    let mut t2 = make_task(2, TaskStatus::Running);
    t2.epic_id = None;
    t2.sub_status = SubStatus::NeedsInput;

    let mut t3 = make_task(3, TaskStatus::Running);
    t3.epic_id = None;
    t3.sub_status = SubStatus::Active;

    app.board.tasks = vec![t1, t2, t3];
    app.board.flattened = true;

    let items = app.column_items_for_status(TaskStatus::Running);

    let separator_count = items
        .iter()
        .filter(|i| matches!(i, ColumnItem::OrphanSeparator))
        .count();
    assert_eq!(
        separator_count, 1,
        "exactly one separator at the NeedsInput epic→orphan transition"
    );
}

// ---------------------------------------------------------------------------
// epic_repo_matches
// ---------------------------------------------------------------------------

#[test]
fn epic_repo_matches_no_filter_always_true() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(1)];
    // No filter active → always true regardless of tasks
    assert!(app.epic_repo_matches(EpicId(1)));
}

#[test]
fn epic_repo_matches_empty_epic_with_active_filter_true() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(1)];
    // Filter active but no tasks → empty epic is always shown
    app.filter.repos = std::collections::HashSet::from(["/other/repo".to_string()]);
    assert!(app.epic_repo_matches(EpicId(1)));
}

#[test]
fn epic_repo_matches_with_matching_task_true() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(1)];
    let mut task = make_task(10, crate::models::TaskStatus::Backlog);
    task.epic_id = Some(EpicId(1));
    task.repo_path = "/my/repo".to_string();
    app.board.tasks = vec![task];
    app.filter.repos = std::collections::HashSet::from(["/my/repo".to_string()]);
    assert!(app.epic_repo_matches(EpicId(1)));
}

#[test]
fn epic_repo_matches_with_no_matching_task_false() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(1)];
    let mut task = make_task(10, crate::models::TaskStatus::Backlog);
    task.epic_id = Some(EpicId(1));
    task.repo_path = "/other/repo".to_string();
    app.board.tasks = vec![task];
    app.filter.repos = std::collections::HashSet::from(["/my/repo".to_string()]);
    assert!(!app.epic_repo_matches(EpicId(1)));
}

#[test]
fn epic_repo_matches_archived_tasks_excluded() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(1)];
    // Only archived task with matching repo → archived tasks don't count,
    // so epic has no "active" tasks → behaves like empty epic → true
    let mut task = make_task(10, crate::models::TaskStatus::Archived);
    task.epic_id = Some(EpicId(1));
    task.repo_path = "/my/repo".to_string();
    app.board.tasks = vec![task];
    app.filter.repos = std::collections::HashSet::from(["/my/repo".to_string()]);
    // No non-archived tasks → always show
    assert!(app.epic_repo_matches(EpicId(1)));
}

#[test]
fn reparent_epic_input_mode_is_normal_initially() {
    let app = App::new(vec![]);
    assert_eq!(app.input.mode, InputMode::Normal);
    // Verify new variants can be constructed (compile-time check)
    let _a = InputMode::ReparentEpic(EpicId(1));
    let _b = InputMode::ConfirmReparentEpic {
        epic_id: EpicId(1),
        new_parent: None,
    };
    let _c = InputMode::ConfirmReparentEpic {
        epic_id: EpicId(1),
        new_parent: Some(EpicId(2)),
    };
}

#[test]
fn start_reparent_sets_mode_and_picker() {
    use crate::tui::messages::EpicMessage;
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    // navigate cursor onto epic 10 (board view, column 0, row 0)
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 0);

    app.update(Message::Epic(EpicMessage::StartReparent(EpicId(10))));

    assert_eq!(app.input.mode, InputMode::ReparentEpic(EpicId(10)));
    assert!(app.interaction.reparent_picker.is_some());
    let picker = app.interaction.reparent_picker.as_ref().unwrap();
    assert_eq!(picker.epic_id, EpicId(10));
}

#[test]
fn reparent_navigate_does_not_panic() {
    use crate::tui::messages::EpicMessage;
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.update(Message::Epic(EpicMessage::StartReparent(EpicId(10))));

    // Navigation should not panic even with an empty or default tree state
    app.update(Message::Epic(EpicMessage::ReparentNavigate(
        crate::tui::types::TreeNav::Down,
    )));
    app.update(Message::Epic(EpicMessage::ReparentNavigate(
        crate::tui::types::TreeNav::Up,
    )));
}

#[test]
fn reparent_confirm_with_no_parent_selected_transitions_to_confirm_mode() {
    use crate::tui::messages::EpicMessage;
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.update(Message::Epic(EpicMessage::StartReparent(EpicId(10))));
    // select_first picks "— no parent —" (our sentinel)
    if let Some(picker) = &app.interaction.reparent_picker {
        picker.tree_state.borrow_mut().select_first();
    }

    app.update(Message::Epic(EpicMessage::ReparentConfirm));

    assert!(
        matches!(
            app.input.mode,
            InputMode::ConfirmReparentEpic {
                epic_id: EpicId(10),
                new_parent: None
            }
        ),
        "expected ConfirmReparentEpic, got {:?}",
        app.input.mode
    );
    assert!(app.status.message.is_some());
}

#[test]
fn reparent_execute_emits_reparent_command_and_resets_state() {
    use crate::tui::messages::EpicMessage;
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.input.mode = InputMode::ConfirmReparentEpic {
        epic_id: EpicId(10),
        new_parent: Some(EpicId(20)),
    };
    app.interaction.reparent_picker = Some(make_reparent_picker(EpicId(10)));

    let cmds = app.update(Message::Epic(EpicMessage::ReparentExecute));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.interaction.reparent_picker.is_none());
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Reparent {
            id: EpicId(10),
            new_parent: Some(EpicId(20)),
        })
    )));
}

#[test]
fn reparent_cancel_from_confirm_returns_to_picker() {
    use crate::tui::messages::EpicMessage;
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmReparentEpic {
        epic_id: EpicId(10),
        new_parent: None,
    };
    app.interaction.reparent_picker = Some(make_reparent_picker(EpicId(10)));

    app.update(Message::Epic(EpicMessage::ReparentCancel));

    assert_eq!(app.input.mode, InputMode::ReparentEpic(EpicId(10)));
    assert!(app.interaction.reparent_picker.is_some());
}

#[test]
fn reparent_cancel_from_picker_clears_state() {
    use crate::tui::messages::EpicMessage;
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ReparentEpic(EpicId(10));
    app.interaction.reparent_picker = Some(make_reparent_picker(EpicId(10)));

    app.update(Message::Epic(EpicMessage::ReparentCancel));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.interaction.reparent_picker.is_none());
}

// ---------------------------------------------------------------------------
// Task 4: Key binding tests
// ---------------------------------------------------------------------------

#[test]
fn m_key_on_epic_card_opens_reparent_picker() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1); // Backlog column (no standalone tasks, epic is at row 0)
    app.selection_mut().set_row(1, 0); // first row — the epic

    app.handle_key(make_key(KeyCode::Char('m')));

    assert_eq!(app.input.mode, InputMode::ReparentEpic(EpicId(10)));
    assert!(app.interaction.reparent_picker.is_some());
}

#[test]
fn m_key_on_task_opens_move_to_epic_picker() {
    // `m` on a task card opens the move-to-epic picker (it reparents an epic
    // when the cursor is on an epic card instead — see
    // `m_key_on_epic_card_opens_reparent_picker`).
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    // cursor on task (row 0), not epic (row 1)
    app.selection_mut().set_column(1); // backlog task column
    app.selection_mut().set_row(1, 0);

    app.handle_key(make_key(KeyCode::Char('m')));

    assert_eq!(app.input.mode, InputMode::MoveTaskToEpic(TaskId(1)));
    assert!(app.interaction.move_task_picker.is_some());
}

#[test]
fn reparent_mode_j_k_dispatch_navigate_messages() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.input.mode = InputMode::ReparentEpic(EpicId(10));
    app.interaction.reparent_picker = Some(make_reparent_picker(EpicId(10)));

    // j and k should navigate, not crash
    app.handle_key(make_key(KeyCode::Char('j')));
    app.handle_key(make_key(KeyCode::Char('k')));
    app.handle_key(make_key(KeyCode::Down));
    app.handle_key(make_key(KeyCode::Up));
    // mode should remain ReparentEpic
    assert_eq!(app.input.mode, InputMode::ReparentEpic(EpicId(10)));
}

#[test]
fn reparent_mode_esc_cancels() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ReparentEpic(EpicId(10));
    app.interaction.reparent_picker = Some(make_reparent_picker(EpicId(10)));

    app.handle_key(make_key(KeyCode::Esc));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.interaction.reparent_picker.is_none());
}

#[test]
fn confirm_reparent_y_executes() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.input.mode = InputMode::ConfirmReparentEpic {
        epic_id: EpicId(10),
        new_parent: Some(EpicId(20)),
    };
    app.interaction.reparent_picker = Some(make_reparent_picker(EpicId(10)));

    let cmds = app.handle_key(make_key(KeyCode::Char('y')));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Epic(crate::tui::commands::EpicCommand::Reparent {
            id: EpicId(10),
            new_parent: Some(EpicId(20)),
        })
    )));
}

#[test]
fn confirm_reparent_n_returns_to_picker() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    app.input.mode = InputMode::ConfirmReparentEpic {
        epic_id: EpicId(10),
        new_parent: None,
    };
    app.interaction.reparent_picker = Some(make_reparent_picker(EpicId(10)));

    app.handle_key(make_key(KeyCode::Char('n')));

    assert_eq!(app.input.mode, InputMode::ReparentEpic(EpicId(10)));
}

#[test]
fn esc_from_confirm_reparent_cancels_entirely() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.input.mode = InputMode::ConfirmReparentEpic {
        epic_id: EpicId(10),
        new_parent: Some(EpicId(20)),
    };
    app.interaction.reparent_picker = Some(make_reparent_picker(EpicId(10)));

    app.handle_key(make_key(KeyCode::Esc));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.interaction.reparent_picker.is_none());
}

#[test]
fn q_from_confirm_reparent_cancels_entirely() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.input.mode = InputMode::ConfirmReparentEpic {
        epic_id: EpicId(10),
        new_parent: Some(EpicId(20)),
    };
    app.interaction.reparent_picker = Some(make_reparent_picker(EpicId(10)));

    app.handle_key(make_key(KeyCode::Char('q')));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.interaction.reparent_picker.is_none());
}

// ---------------------------------------------------------------------------
// reparent_target_epics — picker eligibility filtering (task #1595)
// ---------------------------------------------------------------------------

/// Helper: build an epic with a specific status and parent.
fn epic_with(id: i64, status: TaskStatus, parent: Option<i64>) -> crate::models::Epic {
    crate::models::Epic {
        status,
        parent_epic_id: parent.map(EpicId),
        ..make_epic(id)
    }
}

fn target_ids(app: &App, target: EpicId) -> Vec<i64> {
    app.reparent_target_epics(target)
        .iter()
        .map(|e| e.id.0)
        .collect()
}

#[test]
fn reparent_target_epics_excludes_target_and_descendants() {
    let mut app = App::new(vec![]);
    // 10 (target) -> 11 (child) -> 12 (grandchild); 20 unrelated
    app.board.epics = vec![
        epic_with(10, TaskStatus::Backlog, None),
        epic_with(11, TaskStatus::Backlog, Some(10)),
        epic_with(12, TaskStatus::Backlog, Some(11)),
        epic_with(20, TaskStatus::Backlog, None),
    ];
    let ids = target_ids(&app, EpicId(10));
    assert_eq!(
        ids,
        vec![20],
        "target + descendants excluded, unrelated kept"
    );
}

#[test]
fn reparent_target_epics_excludes_done_and_archived() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        epic_with(10, TaskStatus::Backlog, None), // target
        epic_with(20, TaskStatus::Done, None),
        epic_with(30, TaskStatus::Archived, None),
        epic_with(40, TaskStatus::Backlog, None),
    ];
    let ids = target_ids(&app, EpicId(10));
    assert_eq!(
        ids,
        vec![40],
        "Done and Archived epics are not reparent targets"
    );
}

#[test]
fn reparent_target_epics_excludes_repo_filtered_epic() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        epic_with(10, TaskStatus::Backlog, None), // target
        epic_with(20, TaskStatus::Backlog, None),
        epic_with(30, TaskStatus::Backlog, None),
    ];
    let mut task_b = make_task(1, TaskStatus::Backlog);
    task_b.epic_id = Some(EpicId(20));
    task_b.repo_path = "/repo-b".to_string();
    let mut task_c = make_task(2, TaskStatus::Backlog);
    task_c.epic_id = Some(EpicId(30));
    task_c.repo_path = "/repo-c".to_string();
    app.board.tasks = vec![task_b, task_c];
    app.filter.repos.insert("/repo-b".to_string());

    let ids = target_ids(&app, EpicId(10));
    assert_eq!(ids, vec![20], "epic whose tasks are filtered out is hidden");
}

#[test]
fn reparent_target_epics_excludes_only_active_filtered_epic() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        epic_with(10, TaskStatus::Backlog, None), // target
        epic_with(20, TaskStatus::Running, None),
        epic_with(30, TaskStatus::Running, None),
    ];
    let mut active = make_task(1, TaskStatus::Running);
    active.epic_id = Some(EpicId(20));
    active.tmux_window = Some("sess:1".to_string());
    let mut inactive = make_task(2, TaskStatus::Running);
    inactive.epic_id = Some(EpicId(30));
    inactive.tmux_window = None; // no live session — what only_active filters on
    app.board.tasks = vec![active, inactive];
    app.filter.only_active = true;

    let ids = target_ids(&app, EpicId(10));
    assert_eq!(
        ids,
        vec![20],
        "only_active hides epic with no active descendant"
    );
}

#[test]
fn reparent_target_epics_keeps_eligible_epics() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        epic_with(10, TaskStatus::Backlog, None), // target
        epic_with(20, TaskStatus::Backlog, None),
        epic_with(30, TaskStatus::Running, None),
        epic_with(40, TaskStatus::Review, None),
    ];
    let ids = target_ids(&app, EpicId(10));
    assert_eq!(
        ids,
        vec![20, 30, 40],
        "Backlog/Running/Review epics are all eligible"
    );
}

// ---------------------------------------------------------------------------
// Picker item pre-computation
// ---------------------------------------------------------------------------

#[test]
fn reparent_picker_has_prebuilt_tree_items_on_open() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(1), make_epic(2)];

    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::StartReparent(EpicId(1)),
    ));

    let picker = app
        .interaction
        .reparent_picker
        .as_ref()
        .expect("picker should be set");
    assert!(
        !picker.items.is_empty(),
        "picker items must be prebuilt when picker opens"
    );
    // First item is always the "— no parent —" sentinel
    assert_eq!(
        picker.items[0].identifier(),
        crate::tui::types::REPARENT_NO_PARENT_SENTINEL,
        "first item must be the no-parent sentinel"
    );
}

#[test]
fn move_task_picker_has_prebuilt_tree_items_on_open() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::StartMoveToEpic(TaskId(1)),
    ));

    let picker = app
        .interaction
        .move_task_picker
        .as_ref()
        .expect("picker should be set");
    assert!(
        !picker.items.is_empty(),
        "move-task picker items must be prebuilt when picker opens"
    );
}
