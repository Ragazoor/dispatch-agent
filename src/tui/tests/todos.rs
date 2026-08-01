#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::models::{Todo, TodoId};
use crate::tui::messages::TodoMessage;
use crate::tui::types::{Command, Message, ViewMode};
use crate::tui::App;
use chrono::Utc;

fn make_todo_test_task(id: TaskId, title: &str) -> crate::models::Task {
    use crate::models::*;
    Task {
        id,
        title: title.to_string(),
        description: String::new(),
        repo_path: "/repo".into(),
        status: TaskStatus::Backlog,
        sub_status: SubStatus::None,
        worktree: None,
        tmux_window: None,
        plan_path: None,
        epic_id: None,
        url: None,
        tag: None,
        sort_order: None,
        base_branch: "main".into(),
        external_id: None,
        labels: vec![],
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_pre_tool_use_at: None,
        last_notification_at: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        live_subagents: 0,
        stop_pending: false,
    }
}

use crate::models::TaskId;

fn make_app() -> App {
    App::new(vec![])
}

fn make_todo(id: i64, title: &str, done: bool, sort_order: i64) -> Todo {
    Todo {
        id: TodoId(id),
        title: title.into(),
        done,
        sort_order,
        parent_id: None,
        linked: None,
        created_at: Utc::now(),
    }
}

fn show(app: &mut App, todos: Vec<Todo>) {
    app.update(Message::Todo(TodoMessage::Show(todos)));
}

#[test]
fn open_returns_load_command() {
    let mut app = make_app();
    let cmds = app.update(Message::Todo(TodoMessage::Open));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Todo(crate::tui::commands::TodoCommand::Load))));
}

#[test]
fn show_sets_view_mode_with_done_items_last() {
    let mut app = make_app();
    let todos = vec![
        make_todo(1, "open-a", false, 0),
        make_todo(2, "done-b", true, 1),
        make_todo(3, "open-c", false, 2),
    ];
    app.update(Message::Todo(TodoMessage::Show(todos)));
    match &app.board.view_mode {
        ViewMode::Todos {
            todos, selected, ..
        } => {
            assert_eq!(*selected, 0);
            assert!(!todos[0].done);
            assert!(!todos[1].done);
            assert!(todos[2].done); // done sorted last
        }
        other => panic!("expected Todos view, got {other:?}"),
    }
}

#[test]
fn q_restores_previous_view() {
    let mut app = make_app();
    app.update(Message::Todo(TodoMessage::Show(vec![make_todo(
        1, "x", false, 0,
    )])));
    app.update(Message::Todo(TodoMessage::Close));
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn todo_keys_inert_outside_todos_viewmode() {
    // The in-view 'space' toggle must NOT leak to the board: pressing space on
    // the board emits no TodoCommand. (A vacuous "view_mode unchanged" assertion
    // would pass even with zero todo code — this guards the routing instead.)
    use crossterm::event::{KeyCode, KeyEvent};
    let mut app = make_app();
    let cmds = app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
    assert!(
        !cmds.iter().any(|c| matches!(c, Command::Todo(_))),
        "space on the board must not produce a TodoCommand"
    );
}

#[test]
fn space_toggles_done_on_selected_and_emits_update() {
    use crate::tui::commands::TodoCommand;
    let mut app = make_app();
    show(&mut app, vec![make_todo(1, "x", false, 0)]);
    let cmds = app.update(Message::Todo(TodoMessage::ToggleDone(TodoId(1))));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Todo(TodoCommand::Update { id, update })
            if *id == TodoId(1) && update.done == Some(true)
    )));
    if let ViewMode::Todos { todos, .. } = &app.board.view_mode {
        assert!(todos[0].done);
    } else {
        panic!("expected Todos view");
    }
}

#[test]
fn shift_jk_reorders_within_list_two_updates() {
    use crate::tui::commands::TodoCommand;
    let mut app = make_app();
    show(
        &mut app,
        vec![make_todo(1, "a", false, 0), make_todo(2, "b", false, 1)],
    );
    // selected = 0 (item a). Move down: swap with b.
    let cmds = app.update(Message::Todo(TodoMessage::Reorder(1)));
    let updates: Vec<_> = cmds
        .iter()
        .filter(|c| matches!(c, Command::Todo(TodoCommand::Update { .. })))
        .collect();
    assert_eq!(updates.len(), 2);
    if let ViewMode::Todos {
        todos, selected, ..
    } = &app.board.view_mode
    {
        assert_eq!(todos[0].id, TodoId(2));
        assert_eq!(todos[1].id, TodoId(1));
        assert_eq!(*selected, 1); // selection follows the moved item
    } else {
        panic!("expected Todos view");
    }
}

#[test]
fn clear_done_drops_done_and_emits_command() {
    use crate::tui::commands::TodoCommand;
    let mut app = make_app();
    show(
        &mut app,
        vec![
            make_todo(1, "keep", false, 0),
            make_todo(2, "gone", true, 1),
        ],
    );
    let cmds = app.update(Message::Todo(TodoMessage::ClearDone));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Todo(TodoCommand::ClearDone))));
    if let ViewMode::Todos { todos, .. } = &app.board.view_mode {
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].id, TodoId(1));
    } else {
        panic!("expected Todos view");
    }
}

#[test]
fn delete_drops_selected_and_emits_command() {
    use crate::tui::commands::TodoCommand;
    let mut app = make_app();
    show(&mut app, vec![make_todo(1, "x", false, 0)]);
    let cmds = app.update(Message::Todo(TodoMessage::Delete(TodoId(1))));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Todo(TodoCommand::Delete(id)) if *id == TodoId(1))));
    if let ViewMode::Todos { todos, .. } = &app.board.view_mode {
        assert!(todos.is_empty());
    }
}

#[test]
fn edit_prefills_buffer_from_selected_item() {
    let mut app = make_app();
    show(&mut app, vec![make_todo(1, "edit me", false, 0)]);
    app.update(Message::Todo(TodoMessage::Edit(TodoId(1))));
    assert_eq!(app.input.buffer, "edit me");
    assert!(matches!(
        app.input.mode,
        crate::tui::types::InputMode::TodoTitle
    ));
}

#[test]
fn add_opens_input_mode_todo_title() {
    let mut app = make_app();
    show(&mut app, vec![make_todo(1, "existing", false, 0)]);
    app.update(Message::Todo(TodoMessage::Add));
    assert_eq!(app.input.buffer, "");
    assert!(matches!(
        app.input.mode,
        crate::tui::types::InputMode::TodoTitle
    ));
    assert!(matches!(
        app.interaction.pending,
        crate::tui::PendingAction::None
    ));
}

#[test]
fn d_routes_through_confirm_delete() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = make_app();
    show(&mut app, vec![make_todo(1, "delete me", false, 0)]);
    // Press 'd' — should set ConfirmDeleteTodo mode and store pending id.
    let _ = app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(matches!(
        app.input.mode,
        crate::tui::types::InputMode::ConfirmDeleteTodo
    ));
    assert_eq!(
        app.interaction.pending,
        crate::tui::PendingAction::TodoDelete(TodoId(1))
    );
}

#[test]
fn t_on_board_with_no_selection_is_noop() {
    // With the new pre-fill design, 't' with nothing selected is a no-op.
    use crossterm::event::{KeyCode, KeyEvent};
    let mut app = make_app();
    let _ = app.handle_key(KeyEvent::from(KeyCode::Char('t')));
    assert!(matches!(
        app.input.mode,
        crate::tui::types::InputMode::Normal
    ));
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn t_on_board_with_task_selected_enters_quick_add_mode() {
    use crate::models::TodoLink;
    use crate::tui::types::BoardSelection;
    use crossterm::event::{KeyCode, KeyEvent};
    let task = make_todo_test_task(TaskId(1), "Some task");
    let mut app = App::new(vec![task]);
    app.board.view_mode = ViewMode::Board(BoardSelection::new_for_board());
    app.selection_mut().set_column(1); // Backlog column
    let _ = app.handle_key(KeyEvent::from(KeyCode::Char('t')));
    assert!(matches!(
        app.input.mode,
        crate::tui::types::InputMode::TodoQuickAdd
    ));
    assert_eq!(app.input.buffer, "Some task");
    assert_eq!(
        app.interaction.pending,
        crate::tui::PendingAction::TodoLink(TodoLink::Task(TaskId(1)))
    );
    assert!(matches!(app.board.view_mode, ViewMode::Board(_))); // stays on board
}

#[test]
fn count_updated_sets_board_count() {
    let mut app = make_app();
    app.update(Message::Todo(TodoMessage::CountUpdated(3)));
    assert_eq!(app.board.todo_open_count, 3);
}

#[test]
fn todo_add_mode_shows_buffer_in_overlay() {
    // When the user presses 'a' inside the todos overlay and types text,
    // the typed text must be visible somewhere in the rendered output.
    let mut app = App::new(vec![]);
    app.update(Message::Todo(TodoMessage::Show(vec![make_todo(
        1, "existing", false, 0,
    )])));
    app.update(Message::Todo(TodoMessage::Add));
    app.input.set_buffer("hello world".to_string());
    let buf = super::render_to_buffer(&mut app, 120, 40);
    assert!(
        super::buffer_contains(&buf, "hello world"),
        "typed text should be visible in the todos overlay"
    );
}

#[test]
fn todo_title_mode_status_bar_shows_buffer() {
    // The status bar must include the buffer contents so the user can
    // see what they are typing even before the overlay input row renders.
    let mut app = App::new(vec![]);
    app.update(Message::Todo(TodoMessage::Show(vec![])));
    app.update(Message::Todo(TodoMessage::Add));
    app.input.set_buffer("typing here".to_string());
    let buf = super::render_to_buffer(&mut app, 120, 40);
    assert!(
        super::buffer_contains(&buf, "typing here"),
        "buffer content should appear in the status bar"
    );
}

#[test]
fn todo_quick_add_mode_status_bar_shows_buffer() {
    // TodoQuickAdd (board-level, no overlay) must show the buffer inline
    // in the status bar so the user sees what they are typing.
    let mut app = App::new(vec![]);
    app.input.mode = crate::tui::types::InputMode::TodoQuickAdd;
    app.input.set_buffer("new item".to_string());
    let buf = super::render_to_buffer(&mut app, 120, 40);
    assert!(
        super::buffer_contains(&buf, "new item"),
        "buffer content should appear in the status bar for quick-add mode"
    );
}

#[test]
fn submit_title_while_todos_open_preserves_board_as_previous() {
    // Regression: when Enter is pressed in the add-todo input (TodoTitle mode)
    // while the Todos overlay is open, exec_load_todos calls handle_show_todos
    // again. handle_show_todos must preserve the pre-Todos `previous` view so
    // effective_view_mode() keeps returning Board, not Todos — otherwise the
    // unreachable!() guards in tasks_for_current_view et al. panic.
    let mut app = make_app();
    show(&mut app, vec![make_todo(1, "existing", false, 0)]);
    assert!(matches!(app.board.view_mode, ViewMode::Todos { .. }));

    // Simulate Enter submitting the add form: SubmitTitle calls handle_show_todos
    // a second time (as exec_create_todo with reopen=true would do).
    app.update(Message::Todo(TodoMessage::Show(vec![
        make_todo(1, "existing", false, 0),
        make_todo(2, "new item", false, 1),
    ])));

    // The view must still be Todos, but previous must be Board (not Todos).
    match &app.board.view_mode {
        ViewMode::Todos { previous, .. } => {
            assert!(
                matches!(previous.as_ref(), ViewMode::Board(_)),
                "previous should be Board after re-Show, got {previous:?}"
            );
        }
        other => panic!("expected Todos view, got {other:?}"),
    }

    // effective_view_mode must return Board so callers don't hit unreachable!()
    assert!(
        matches!(
            app.effective_view_mode(),
            crate::tui::types::BoardViewMode::Board(_)
        ),
        "effective_view_mode should return Board"
    );
}

#[test]
fn status_bar_shows_count_suffix_only_when_nonzero() {
    // When todo_open_count == 0, no "(N)" count suffix appears in the status bar.
    // When todo_open_count == 2, "(2)" appears in the status bar.
    let mut app = make_app();
    app.board.todo_open_count = 0;
    let buf = super::render_to_buffer(&mut app, 160, 20);
    assert!(
        !super::buffer_contains(&buf, "("),
        "status bar should not show a count suffix when count is 0"
    );

    app.board.todo_open_count = 2;
    let buf = super::render_to_buffer(&mut app, 160, 20);
    assert!(
        super::buffer_contains(&buf, "(2)"),
        "status bar should show '(2)' when todo_open_count is 2"
    );
}

#[test]
fn quick_add_with_task_selected_prefills_buffer_and_stores_link() {
    use crate::models::TodoLink;
    use crate::tui::types::BoardSelection;
    let mut app = App::new(vec![]);
    // Put a task on the board
    let task = make_todo_test_task(TaskId(42), "My Task");
    app.board.tasks = vec![task];
    // Navigate to column 1 (Backlog), row 0
    app.board.view_mode = ViewMode::Board(BoardSelection::new_for_board());
    app.selection_mut().set_column(1);

    let cmds = app.update(Message::Todo(TodoMessage::QuickAdd {
        title: "My Task".to_string(),
        linked: Some(TodoLink::Task(TaskId(42))),
    }));

    assert!(cmds.is_empty());
    assert_eq!(app.input.buffer, "My Task");
    assert_eq!(app.input.mode, crate::tui::types::InputMode::TodoQuickAdd);
    assert_eq!(
        app.interaction.pending,
        crate::tui::PendingAction::TodoLink(TodoLink::Task(TaskId(42)))
    );
}

#[test]
fn quick_add_submit_passes_link_to_create_command() {
    use crate::models::TodoLink;
    let mut app = App::new(vec![]);
    app.input.mode = crate::tui::types::InputMode::TodoQuickAdd;
    app.interaction.pending = crate::tui::PendingAction::TodoLink(TodoLink::Task(TaskId(7)));

    let cmds = app.update(Message::Todo(TodoMessage::SubmitQuickAdd(
        "Buy milk".to_string(),
    )));

    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        Command::Todo(crate::tui::commands::TodoCommand::Create {
            title,
            linked,
            reopen,
        }) => {
            assert_eq!(title, "Buy milk");
            assert_eq!(*linked, Some(TodoLink::Task(TaskId(7))));
            assert!(!reopen);
        }
        other => panic!("expected Create command, got {other:?}"),
    }
}

#[test]
fn l_in_todos_view_enters_link_mode_and_closes_overlay() {
    let mut app = App::new(vec![]);
    let todos = vec![make_todo(1, "Link me", false, 0)];
    app.update(Message::Todo(TodoMessage::Show(todos)));
    assert!(matches!(app.board.view_mode, ViewMode::Todos { .. }));

    let cmds = app.update(Message::Todo(TodoMessage::LinkToTask(TodoId(1))));

    assert!(cmds.is_empty());
    assert!(
        matches!(
            app.board.view_mode,
            ViewMode::Board(_) | ViewMode::Epic { .. }
        ),
        "expected board view after entering link mode, got {:?}",
        app.board.view_mode
    );
    assert_eq!(
        app.input.mode,
        crate::tui::types::InputMode::LinkTodoToTask(TodoId(1))
    );
}

#[test]
fn esc_in_link_mode_cancels_and_reloads_todos() {
    use crate::tui::commands::TodoCommand;
    let mut app = App::new(vec![]);
    app.board.view_mode = ViewMode::Board(crate::tui::types::BoardSelection::new_for_board());
    app.input.mode = crate::tui::types::InputMode::LinkTodoToTask(TodoId(5));

    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    );
    let cmds = app.handle_key(key);

    assert_eq!(app.input.mode, crate::tui::types::InputMode::Normal);
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::Todo(TodoCommand::Load))),
        "expected Load command on cancel"
    );
}

#[test]
fn enter_in_link_mode_with_task_focused_emits_update_and_load() {
    use crate::models::TodoLink;
    use crate::tui::commands::TodoCommand;
    let mut app = App::new(vec![]);
    let task = make_todo_test_task(TaskId(99), "Target Task");
    app.board.tasks = vec![task];
    app.board.view_mode = ViewMode::Board(crate::tui::types::BoardSelection::new_for_board());
    app.selection_mut().set_column(1); // Backlog column
    app.input.mode = crate::tui::types::InputMode::LinkTodoToTask(TodoId(3));

    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    );
    let cmds = app.handle_key(key);

    assert_eq!(app.input.mode, crate::tui::types::InputMode::Normal);
    let has_update = cmds.iter().any(|c| {
        matches!(
            c,
            Command::Todo(TodoCommand::Update { update, .. })
            if update.linked == Some(Some(TodoLink::Task(TaskId(99))))
        )
    });
    assert!(has_update, "expected Update command with Task(99) link");
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Todo(TodoCommand::Load))));
}

#[test]
fn u_on_linked_todo_unlinks_and_emits_update() {
    use crate::models::TodoLink;
    use crate::tui::commands::TodoCommand;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = make_app();
    let mut todo = make_todo(1, "linked item", false, 0);
    todo.linked = Some(TodoLink::Task(TaskId(42)));
    show(&mut app, vec![todo]);

    let cmds = app.handle_key(KeyEvent::new(KeyCode::Char('U'), KeyModifiers::NONE));
    let has_update = cmds.iter().any(|c| {
        matches!(
            c,
            Command::Todo(TodoCommand::Update { id, update })
                if *id == TodoId(1) && update.linked == Some(None)
        )
    });
    assert!(has_update, "expected Update command clearing the link");

    // Optimistic in-memory clear
    if let ViewMode::Todos { todos, .. } = &app.board.view_mode {
        assert!(
            todos[0].linked.is_none(),
            "linked should be cleared optimistically"
        );
    } else {
        panic!("expected Todos view");
    }
}

#[test]
fn u_on_unlinked_todo_is_noop() {
    use crate::tui::commands::TodoCommand;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = make_app();
    let todo = make_todo(1, "unlinked item", false, 0); // linked = None
    show(&mut app, vec![todo]);

    let cmds = app.handle_key(KeyEvent::new(KeyCode::Char('U'), KeyModifiers::NONE));
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, Command::Todo(TodoCommand::Update { .. }))),
        "U on an unlinked todo must not emit an Update command"
    );
}

#[test]
fn enter_on_linked_todo_closes_overlay_and_sets_anchor() {
    use crate::models::TodoLink;
    use crate::tui::types::ColumnAnchor;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = make_app();
    let mut todo = make_todo(5, "linked item", false, 0);
    todo.linked = Some(TodoLink::Task(TaskId(99)));
    show(&mut app, vec![todo]);
    assert!(matches!(app.board.view_mode, ViewMode::Todos { .. }));

    let _cmds = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    // Overlay should be closed (reverted to board)
    assert!(
        matches!(
            app.board.view_mode,
            ViewMode::Board(_) | ViewMode::Epic { .. }
        ),
        "expected board view after jump, got {:?}",
        app.board.view_mode
    );
    // Anchor should be set to the linked task
    assert_eq!(
        app.selection().anchor,
        Some(ColumnAnchor::Task(TaskId(99))),
        "anchor should point to the linked task"
    );
}

#[test]
fn esc_during_quick_add_clears_pending_link() {
    use crate::models::TodoLink;
    let mut app = App::new(vec![]);
    // Populate quick-add state as 't' on a board task would
    app.update(Message::Todo(TodoMessage::QuickAdd {
        title: "some title".to_string(),
        linked: Some(TodoLink::Task(TaskId(1))),
    }));
    assert_eq!(
        app.interaction.pending,
        crate::tui::PendingAction::TodoLink(TodoLink::Task(TaskId(1))),
        "pending link should be set after QuickAdd"
    );

    // Press Esc to cancel
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    );
    app.handle_key(key);

    assert_eq!(
        app.interaction.pending,
        crate::tui::PendingAction::None,
        "pending link must be cleared on Esc"
    );
}

#[test]
fn sort_todos_interleaves_children_after_parent() {
    // After Show, the display list should be: parent, child1, child2 (by sort_order).
    let mut app = make_app();
    let parent = Todo {
        id: TodoId(10),
        title: "Parent".into(),
        done: false,
        sort_order: 0,
        parent_id: None,
        linked: None,
        created_at: Utc::now(),
    };
    let child1 = Todo {
        id: TodoId(11),
        title: "Child A".into(),
        done: false,
        sort_order: 0,
        parent_id: Some(TodoId(10)),
        linked: None,
        created_at: Utc::now(),
    };
    let child2 = Todo {
        id: TodoId(12),
        title: "Child B".into(),
        done: false,
        sort_order: 1,
        parent_id: Some(TodoId(10)),
        linked: None,
        created_at: Utc::now(),
    };
    // Pass in reverse order to verify sort_todos normalises it
    app.update(Message::Todo(TodoMessage::Show(vec![
        child2.clone(),
        parent.clone(),
        child1.clone(),
    ])));
    if let ViewMode::Todos { todos, .. } = &app.board.view_mode {
        assert_eq!(todos[0].id, TodoId(10), "parent first");
        assert_eq!(todos[1].id, TodoId(11), "child1 second (lower sort_order)");
        assert_eq!(todos[2].id, TodoId(12), "child2 third");
    } else {
        panic!("expected Todos view");
    }
}

#[test]
fn sort_todos_done_children_after_open_children() {
    let mut app = make_app();
    let parent = Todo {
        id: TodoId(1),
        title: "P".into(),
        done: false,
        sort_order: 0,
        parent_id: None,
        linked: None,
        created_at: Utc::now(),
    };
    let open_child = Todo {
        id: TodoId(2),
        title: "open".into(),
        done: false,
        sort_order: 1,
        parent_id: Some(TodoId(1)),
        linked: None,
        created_at: Utc::now(),
    };
    let done_child = Todo {
        id: TodoId(3),
        title: "done".into(),
        done: true,
        sort_order: 0,
        parent_id: Some(TodoId(1)),
        linked: None,
        created_at: Utc::now(),
    };
    app.update(Message::Todo(TodoMessage::Show(vec![
        done_child, open_child, parent,
    ])));
    if let ViewMode::Todos { todos, .. } = &app.board.view_mode {
        assert_eq!(todos[0].id, TodoId(1), "parent");
        assert_eq!(todos[1].id, TodoId(2), "open child before done child");
        assert_eq!(
            todos[2].id,
            TodoId(3),
            "done child last within parent group"
        );
    } else {
        panic!("expected Todos view");
    }
}

#[test]
fn jk_reorder_root_skips_children() {
    use crate::tui::commands::TodoCommand;
    let mut app = make_app();
    // Display: RootA, ChildA, RootB
    let root_a = Todo {
        id: TodoId(1),
        title: "Root A".into(),
        done: false,
        sort_order: 0,
        parent_id: None,
        linked: None,
        created_at: Utc::now(),
    };
    let child_a = Todo {
        id: TodoId(2),
        title: "Child A".into(),
        done: false,
        sort_order: 0,
        parent_id: Some(TodoId(1)),
        linked: None,
        created_at: Utc::now(),
    };
    let root_b = Todo {
        id: TodoId(3),
        title: "Root B".into(),
        done: false,
        sort_order: 1,
        parent_id: None,
        linked: None,
        created_at: Utc::now(),
    };
    show(&mut app, vec![root_a, child_a, root_b]);

    // Cursor is on Root A (index 0). Press J → should swap with Root B.
    let cmds = app.update(Message::Todo(TodoMessage::Reorder(1)));

    // Two update commands: one for Root A, one for Root B.
    let updates: Vec<_> = cmds
        .iter()
        .filter(|c| matches!(c, Command::Todo(TodoCommand::Update { .. })))
        .collect();
    assert_eq!(updates.len(), 2);

    if let ViewMode::Todos {
        todos, selected, ..
    } = &app.board.view_mode
    {
        // Root B is now first (it swapped with Root A)
        assert_eq!(todos[0].id, TodoId(3), "Root B should be first");
        // Child A still follows Root A
        assert_eq!(todos[1].id, TodoId(1), "Root A second");
        assert_eq!(todos[2].id, TodoId(2), "Child A still under Root A");
        // Selection follows Root A
        assert_eq!(*selected, 1);
    } else {
        panic!("expected Todos view");
    }
}

#[test]
fn jk_reorder_child_swaps_with_sibling_only() {
    use crate::tui::commands::TodoCommand;
    let mut app = make_app();
    let parent = Todo {
        id: TodoId(1),
        title: "Parent".into(),
        done: false,
        sort_order: 0,
        parent_id: None,
        linked: None,
        created_at: Utc::now(),
    };
    let child1 = Todo {
        id: TodoId(2),
        title: "C1".into(),
        done: false,
        sort_order: 0,
        parent_id: Some(TodoId(1)),
        linked: None,
        created_at: Utc::now(),
    };
    let child2 = Todo {
        id: TodoId(3),
        title: "C2".into(),
        done: false,
        sort_order: 1,
        parent_id: Some(TodoId(1)),
        linked: None,
        created_at: Utc::now(),
    };
    show(&mut app, vec![parent, child1, child2]);

    // Move cursor to child1 (index 1) and press J
    app.update(Message::Todo(TodoMessage::MoveSelection(1)));
    let cmds = app.update(Message::Todo(TodoMessage::Reorder(1)));

    let updates: Vec<_> = cmds
        .iter()
        .filter(|c| matches!(c, Command::Todo(TodoCommand::Update { .. })))
        .collect();
    assert_eq!(updates.len(), 2);

    if let ViewMode::Todos {
        todos, selected, ..
    } = &app.board.view_mode
    {
        assert_eq!(todos[0].id, TodoId(1), "Parent still first");
        assert_eq!(todos[1].id, TodoId(3), "C2 now before C1");
        assert_eq!(todos[2].id, TodoId(2), "C1 now after C2");
        assert_eq!(*selected, 2, "selection follows C1");
    } else {
        panic!("expected Todos view");
    }
}

#[test]
fn jk_reorder_noop_at_sibling_boundary() {
    let mut app = make_app();
    let parent = Todo {
        id: TodoId(1),
        title: "P".into(),
        done: false,
        sort_order: 0,
        parent_id: None,
        linked: None,
        created_at: Utc::now(),
    };
    let child = Todo {
        id: TodoId(2),
        title: "C".into(),
        done: false,
        sort_order: 0,
        parent_id: Some(TodoId(1)),
        linked: None,
        created_at: Utc::now(),
    };
    show(&mut app, vec![parent, child]);

    // Cursor on child (index 1). Press J — no sibling below → no-op.
    app.update(Message::Todo(TodoMessage::MoveSelection(1)));
    let cmds = app.update(Message::Todo(TodoMessage::Reorder(1)));
    assert!(
        cmds.is_empty(),
        "should be no-op when no sibling in that direction"
    );
}

#[test]
fn nest_makes_selected_todo_child_of_root_above() {
    use crate::tui::commands::TodoCommand;
    let mut app = make_app();
    show(
        &mut app,
        vec![
            make_todo(1, "Parent", false, 0),
            make_todo(2, "To nest", false, 1),
        ],
    );
    // Move cursor to item 2 (index 1)
    app.update(Message::Todo(TodoMessage::MoveSelection(1)));
    let cmds = app.update(Message::Todo(TodoMessage::Nest(TodoId(2))));

    // Should emit one Update setting parent_id = Some(TodoId(1))
    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        Command::Todo(TodoCommand::Update { id, update }) => {
            assert_eq!(*id, TodoId(2));
            assert_eq!(update.parent_id, Some(Some(TodoId(1))));
        }
        other => panic!("expected Update, got {other:?}"),
    }
    // In-memory: item 2 is now a child of item 1
    if let ViewMode::Todos { todos, .. } = &app.board.view_mode {
        let nested = todos.iter().find(|t| t.id == TodoId(2)).unwrap();
        assert_eq!(nested.parent_id, Some(TodoId(1)));
    }
}

#[test]
fn nest_noop_when_already_a_child() {
    let mut app = make_app();
    let child = Todo {
        id: TodoId(2),
        title: "Already child".into(),
        done: false,
        sort_order: 0,
        parent_id: Some(TodoId(1)),
        linked: None,
        created_at: Utc::now(),
    };
    let parent = make_todo(1, "Parent", false, 0);
    show(&mut app, vec![parent, child]);
    // Move to child (index 1)
    app.update(Message::Todo(TodoMessage::MoveSelection(1)));
    let cmds = app.update(Message::Todo(TodoMessage::Nest(TodoId(2))));
    assert!(
        cmds.is_empty(),
        "nesting an already-nested item must be a no-op"
    );
}

#[test]
fn nest_noop_when_no_root_above() {
    let mut app = make_app();
    show(&mut app, vec![make_todo(1, "Only item", false, 0)]);
    let cmds = app.update(Message::Todo(TodoMessage::Nest(TodoId(1))));
    assert!(cmds.is_empty(), "nesting the first item must be a no-op");
}

#[test]
fn unnest_promotes_child_to_root() {
    use crate::tui::commands::TodoCommand;
    let mut app = make_app();
    let parent = make_todo(1, "Parent", false, 0);
    let child = Todo {
        id: TodoId(2),
        title: "Child".into(),
        done: false,
        sort_order: 0,
        parent_id: Some(TodoId(1)),
        linked: None,
        created_at: Utc::now(),
    };
    show(&mut app, vec![parent, child]);
    // Move to child (index 1)
    app.update(Message::Todo(TodoMessage::MoveSelection(1)));
    let cmds = app.update(Message::Todo(TodoMessage::Unnest(TodoId(2))));

    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        Command::Todo(TodoCommand::Update { id, update }) => {
            assert_eq!(*id, TodoId(2));
            assert_eq!(update.parent_id, Some(None));
        }
        other => panic!("expected Update, got {other:?}"),
    }
    // In-memory: item 2 is now root
    if let ViewMode::Todos { todos, .. } = &app.board.view_mode {
        let promoted = todos.iter().find(|t| t.id == TodoId(2)).unwrap();
        assert_eq!(promoted.parent_id, None);
    }
}

#[test]
fn unnest_noop_when_already_root() {
    let mut app = make_app();
    show(&mut app, vec![make_todo(1, "Root item", false, 0)]);
    let cmds = app.update(Message::Todo(TodoMessage::Unnest(TodoId(1))));
    assert!(cmds.is_empty(), "unnesting a root item must be a no-op");
}

#[test]
fn tab_key_in_todos_view_nests_selected() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = make_app();
    show(
        &mut app,
        vec![
            make_todo(1, "Parent", false, 0),
            make_todo(2, "To nest", false, 1),
        ],
    );
    app.update(Message::Todo(TodoMessage::MoveSelection(1)));
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Todo(crate::tui::commands::TodoCommand::Update { .. })
        )),
        "Tab should emit an Update command (nest)"
    );
}

#[test]
fn shift_tab_key_in_todos_view_unnests_selected() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = make_app();
    let parent = make_todo(1, "Parent", false, 0);
    let child = Todo {
        id: TodoId(2),
        title: "Child".into(),
        done: false,
        sort_order: 0,
        parent_id: Some(TodoId(1)),
        linked: None,
        created_at: Utc::now(),
    };
    show(&mut app, vec![parent, child]);
    app.update(Message::Todo(TodoMessage::MoveSelection(1)));
    let cmds = app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Todo(crate::tui::commands::TodoCommand::Update { .. })
        )),
        "Shift+Tab should emit an Update command (unnest)"
    );
}

#[test]
fn jump_to_linked_task_in_epic_enters_epic_view() {
    use crate::models::{EpicId, TodoLink};
    use crate::tui::types::ColumnAnchor;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    // Task 10 belongs to epic 1.
    let mut task = make_todo_test_task(TaskId(10), "epic task");
    task.epic_id = Some(EpicId(1));

    let mut app = App::new(vec![task]);
    app.board.epics.push(super::make_epic(1));

    let mut todo = make_todo(1, "linked to epic task", false, 0);
    todo.linked = Some(TodoLink::Task(TaskId(10)));
    show(&mut app, vec![todo]);
    assert!(matches!(app.board.view_mode, ViewMode::Todos { .. }));

    let _cmds = app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));

    // Should have entered the epic view
    assert!(
        matches!(app.board.view_mode, ViewMode::Epic { epic_id, .. } if epic_id == EpicId(1)),
        "expected Epic view for epic 1, got {:?}",
        app.board.view_mode
    );
    // Anchor should point to the linked task
    assert_eq!(
        app.selection().anchor,
        Some(ColumnAnchor::Task(TaskId(10))),
        "anchor should point to the linked task inside the epic"
    );
}

#[test]
fn enter_on_unlinked_todo_is_noop() {
    use crate::tui::commands::TodoCommand;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = make_app();
    let todo = make_todo(1, "unlinked item", false, 0); // linked = None
    show(&mut app, vec![todo]);

    let cmds = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    // No commands, overlay stays open
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, Command::Todo(TodoCommand::Update { .. }))),
        "Enter on unlinked todo must not emit an Update command"
    );
    assert!(
        matches!(app.board.view_mode, ViewMode::Todos { .. }),
        "view mode should remain Todos when Enter is pressed on an unlinked todo"
    );
}
