#![allow(clippy::unwrap_used, clippy::expect_used)]
use crossterm::event::KeyCode;

use super::super::{App, Command, InputMode, Message};
use super::{make_app, make_epic, make_key, make_task};
use crate::models::{TaskId, TaskStatus};

/// Drives an `App` through a sequence of key events, collecting all `Command`s emitted.
struct Scenario {
    app: App,
    commands: Vec<Command>,
}

impl Scenario {
    fn new() -> Self {
        Self {
            app: make_app(),
            commands: vec![],
        }
    }

    fn with_app(app: App) -> Self {
        Self {
            app,
            commands: vec![],
        }
    }

    fn key(&mut self, code: KeyCode) -> &mut Self {
        let cmds = self.app.handle_key(make_key(code));
        self.commands.extend(cmds);
        self
    }

    fn char_keys(&mut self, s: &str) -> &mut Self {
        for c in s.chars() {
            self.key(KeyCode::Char(c));
        }
        self
    }
}

#[test]
fn scenario_task_creation_dialog_enters_input_title_mode() {
    let mut s = Scenario::new();
    s.key(KeyCode::Char('n'));
    assert!(
        matches!(s.app.input.mode, InputMode::InputTitle),
        "expected InputTitle after pressing n, got {:?}",
        s.app.input.mode
    );
}

#[test]
fn scenario_task_creation_empty_title_returns_to_normal() {
    let mut s = Scenario::new();
    s.key(KeyCode::Char('n'));
    s.key(KeyCode::Enter);
    assert!(
        matches!(s.app.input.mode, InputMode::Normal),
        "empty title should return to Normal, got {:?}",
        s.app.input.mode
    );
}

#[test]
fn scenario_task_creation_typing_title_advances_to_tag_input() {
    let mut s = Scenario::new();
    s.key(KeyCode::Char('n'));
    s.char_keys("My Task");
    s.key(KeyCode::Enter);
    assert!(
        matches!(s.app.input.mode, InputMode::InputTag),
        "expected InputTag after submitting title, got {:?}",
        s.app.input.mode
    );
}

#[test]
fn scenario_task_creation_esc_cancels_from_title_input() {
    let mut s = Scenario::new();
    s.key(KeyCode::Char('n'));
    s.char_keys("partial");
    s.key(KeyCode::Esc);
    assert!(
        matches!(s.app.input.mode, InputMode::Normal),
        "Esc should cancel back to Normal, got {:?}",
        s.app.input.mode
    );
    assert!(
        s.app.input.buffer.is_empty(),
        "buffer should be cleared after cancel"
    );
}

#[test]
fn scenario_quick_dispatch_with_repo_path_emits_command() {
    let mut app = make_app();
    app.update(Message::RepoPathsUpdated(vec!["/repo".to_string()]));

    let mut s = Scenario::with_app(app);
    s.key(KeyCode::Char('D'));

    assert!(
        s.commands.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { .. })
        )),
        "expected QuickDispatch command, got {:?}",
        s.commands
    );
}

#[test]
fn scenario_quick_dispatch_without_repo_path_shows_no_dispatch_command() {
    let mut s = Scenario::new();
    s.key(KeyCode::Char('D'));

    assert!(
        !s.commands.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { .. })
        )),
        "should not emit QuickDispatch without a repo path"
    );
}

#[test]
fn scenario_help_overlay_opens_on_question_mark() {
    let mut s = Scenario::new();
    s.key(KeyCode::Char('?'));
    assert!(
        matches!(s.app.input.mode, InputMode::Help),
        "expected Help mode after '?', got {:?}",
        s.app.input.mode
    );
}

#[test]
fn scenario_help_overlay_toggles_closed_on_second_question_mark() {
    let mut s = Scenario::new();
    s.key(KeyCode::Char('?'));
    s.key(KeyCode::Char('?'));
    assert!(
        matches!(s.app.input.mode, InputMode::Normal),
        "expected Normal mode after closing help, got {:?}",
        s.app.input.mode
    );
}

#[test]
fn scenario_move_key_advances_selected_task_to_next_column() {
    // Backlog column is selected by default; Shift+L moves task 1 forward (Backlog → Running).
    let mut s = Scenario::new();
    s.key(KeyCode::Char('L'));

    let task1 = s
        .app
        .board
        .tasks
        .iter()
        .find(|t| t.id == TaskId(1))
        .expect("task 1 should exist");
    assert_eq!(
        task1.status,
        TaskStatus::Running,
        "task 1 should be moved to Running after Shift+L"
    );
    assert!(
        s.commands.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Persist(_))
        )),
        "move should emit PersistTask command"
    );
}

#[test]
fn scenario_lowercase_m_is_no_longer_bound_to_move() {
    let mut s = Scenario::new();
    s.key(KeyCode::Char('m'));

    let task1 = s
        .app
        .board
        .tasks
        .iter()
        .find(|t| t.id == TaskId(1))
        .expect("task 1 should exist");
    assert_eq!(
        task1.status,
        TaskStatus::Backlog,
        "lowercase 'm' must not move tasks; it is unbound"
    );
    assert!(
        !s.commands.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Persist(_))
        )),
        "lowercase 'm' must not emit PersistTask"
    );
}

// ---------------------------------------------------------------------------
// Feed epic manual trigger — key binding scenarios
// ---------------------------------------------------------------------------

fn make_feed_epic(id: i64) -> crate::models::Epic {
    let mut e = make_epic(id);
    e.feed_command = Some("echo '[]'".to_string());
    e
}

fn make_app_with_feed_epic_selected() -> super::App {
    use super::App;
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_feed_epic(10)];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1);
    app
}

fn make_app_with_non_feed_epic_selected() -> super::App {
    use super::App;
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)]; // no feed_command
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1);
    app
}

#[test]
fn r_on_feed_epic_card_emits_trigger_command() {
    let mut s = Scenario::with_app(make_app_with_feed_epic_selected());
    s.key(KeyCode::Char('r'));
    assert!(
        s.commands.iter().any(|c| matches!(
            c,
            Command::Feed(crate::tui::commands::FeedCommand::TriggerEpic { .. })
        )),
        "pressing r on a feed epic card should emit TriggerEpicFeed command"
    );
}

#[test]
fn r_on_non_feed_epic_card_does_nothing() {
    let mut s = Scenario::with_app(make_app_with_non_feed_epic_selected());
    s.key(KeyCode::Char('r'));
    assert!(
        !s.commands.iter().any(|c| matches!(
            c,
            Command::Feed(crate::tui::commands::FeedCommand::TriggerEpic { .. })
        )),
        "pressing r on a non-feed epic should NOT emit TriggerEpicFeed"
    );
}

#[test]
fn r_in_epic_view_of_feed_epic_emits_trigger_command() {
    use super::App;
    use crate::tui::{BoardSelection, ViewMode};

    let mut app = App::new(vec![]);
    app.board.epics = vec![make_feed_epic(10)];
    app.board.view_mode = ViewMode::Epic {
        epic_id: crate::models::EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };

    let mut s = Scenario::with_app(app);
    s.key(KeyCode::Char('r'));
    assert!(
        s.commands.iter().any(|c| matches!(
            c,
            Command::Feed(crate::tui::commands::FeedCommand::TriggerEpic { .. })
        )),
        "pressing r inside Epic view of a feed epic should emit TriggerEpicFeed"
    );
}

#[test]
fn r_in_epic_view_of_non_feed_epic_does_nothing() {
    use super::App;
    use crate::tui::{BoardSelection, ViewMode};

    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)]; // no feed_command
    app.board.view_mode = ViewMode::Epic {
        epic_id: crate::models::EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };

    let mut s = Scenario::with_app(app);
    s.key(KeyCode::Char('r'));
    assert!(
        !s.commands.iter().any(|c| matches!(
            c,
            Command::Feed(crate::tui::commands::FeedCommand::TriggerEpic { .. })
        )),
        "pressing r inside Epic view of a non-feed epic should NOT emit TriggerEpicFeed"
    );
}

#[test]
fn search_narrows_board_to_matching_titles() {
    fn titled_task(id: i64, title: &str) -> crate::models::Task {
        let mut t = make_task(id, TaskStatus::Backlog);
        t.title = title.to_string();
        t
    }

    let app = App::new(vec![
        titled_task(1, "Fix login bug"),
        titled_task(2, "Add search feature"),
        titled_task(3, "Refactor parser"),
    ]);

    let mut s = Scenario::with_app(app);
    s.key(KeyCode::Char('/')).char_keys("sea");

    let titles: Vec<String> = s
        .app
        .tasks_for_current_view()
        .iter()
        .map(|t| t.title.clone())
        .collect();
    assert_eq!(titles, vec!["Add search feature".to_string()]);
}

#[test]
fn search_narrows_epic_cards_and_esc_restores_them() {
    use super::{make_epic_with_title, App};
    use crossterm::event::KeyCode;

    fn visible_epic_ids(app: &App) -> Vec<i64> {
        let mut ids: Vec<i64> = app
            .visible_epics_for_effective_view()
            .map(|e| e.id.0)
            .collect();
        ids.sort_unstable();
        ids
    }

    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];

    let mut s = Scenario::with_app(app);
    s.key(KeyCode::Char('/')).char_keys("login");
    assert_eq!(
        visible_epic_ids(&s.app),
        vec![1],
        "board narrows to matching epics while typing"
    );

    // Enter closes the bar but keeps the query active.
    s.key(KeyCode::Enter);
    assert_eq!(visible_epic_ids(&s.app), vec![1], "query persists on Enter");

    // Esc on the board clears the query.
    s.key(KeyCode::Esc);
    assert_eq!(
        visible_epic_ids(&s.app),
        vec![1, 2],
        "clearing the query restores every epic card"
    );
}
