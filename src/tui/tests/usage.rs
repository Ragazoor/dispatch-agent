//! Keybinding telemetry: which key surfaces record a `RecordUsageEvent`.
//!
//! Every arm the TUI acts on records exactly one keybinding usage event, and a
//! keypress that changes nothing records none — see rules
//! `KeypressRecordsFeatureUsage` and `FeatureUsageLogStaysBounded` in
//! `docs/specs/observability.allium`. These tests are per-arm on purpose: the
//! whole point of the instrumentation is that a future pruning pass can trust
//! the absence of a count, so an arm silently losing its push has to read as a
//! regression.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{EpicId, TaskId, TaskStatus, TodoLink, UsageActor, UsageCategory};
use crate::tui::messages::{EpicMessage, TaskMessage, TodoMessage};
use crate::tui::types::{InputMode, TaskDraft};
use crossterm::event::KeyCode;

// ── assertion helpers ────────────────────────────────────────────────────────

fn usage_events(cmds: &[Command]) -> Vec<(String, Option<String>)> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::RecordUsageEvent(e) => Some((e.action.clone(), e.detail.clone())),
            _ => None,
        })
        .collect()
}

/// Press `code` and assert it recorded exactly one event with this action/key.
#[track_caller]
fn assert_records(app: &mut App, code: KeyCode, action: &str, detail: &str) {
    let cmds = app.handle_key(make_key(code));
    assert_eq!(
        usage_events(&cmds),
        vec![(action.to_string(), Some(detail.to_string()))],
        "pressing {code:?} should record {action}/{detail} exactly once"
    );
}

/// Press `code` and assert it recorded nothing at all.
#[track_caller]
fn assert_silent(app: &mut App, code: KeyCode) {
    let cmds = app.handle_key(make_key(code));
    assert_eq!(
        usage_events(&cmds),
        vec![],
        "pressing {code:?} is a no-op and must not record usage"
    );
}

// ── app fixtures ─────────────────────────────────────────────────────────────

fn epic_app() -> App {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Epic(EpicMessage::Enter(EpicId(10))));
    app
}

fn todos_app() -> App {
    let mut app = make_app();
    app.update(Message::Todo(TodoMessage::Show(vec![
        make_todo(1, "first"),
        make_todo(2, "second"),
    ])));
    app
}

fn empty_todos_app() -> App {
    let mut app = make_app();
    app.update(Message::Todo(TodoMessage::Show(vec![])));
    app
}

fn linked_todos_app() -> App {
    let mut app = make_app();
    let mut todo = make_todo(1, "linked");
    todo.linked = Some(TodoLink::Task(TaskId(1)));
    app.update(Message::Todo(TodoMessage::Show(vec![todo])));
    app
}

fn detail_app() -> App {
    let mut app = make_app();
    app.update(Message::Task(TaskMessage::OpenDetail(TaskId(1))));
    app
}

fn archive_app() -> App {
    let mut app = make_app_with_archived_task();
    app.selection_mut().set_column(5); // Archive column
    app
}

fn app_in_mode(mode: InputMode) -> App {
    let mut app = make_app();
    app.input.mode = mode;
    app
}

// ── the event shape itself ───────────────────────────────────────────────────

#[test]
fn recorded_event_is_a_human_keybinding_event() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    let event = cmds
        .iter()
        .find_map(|c| match c {
            Command::RecordUsageEvent(e) => Some(e.clone()),
            _ => None,
        })
        .expect("expected RecordUsageEvent for 'n'");
    assert_eq!(event.category, UsageCategory::Keybinding);
    assert_eq!(event.actor, UsageActor::Human);
    assert_eq!(event.action, "create_task");
    assert_eq!(event.detail.as_deref(), Some("n"));
}

// ── board navigation ─────────────────────────────────────────────────────────

#[test]
fn column_navigation_records_the_key_that_drove_it() {
    let mut app = make_app();
    assert_records(&mut app, KeyCode::Char('l'), "navigate_column", "l");
    assert_records(&mut app, KeyCode::Char('h'), "navigate_column", "h");
    assert_records(&mut app, KeyCode::Right, "navigate_column", "Right");
    assert_records(&mut app, KeyCode::Left, "navigate_column", "Left");
}

#[test]
fn row_navigation_records_the_key_that_drove_it() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    assert_records(&mut app, KeyCode::Char('j'), "navigate_row", "j");
    assert_records(&mut app, KeyCode::Char('k'), "navigate_row", "k");
    assert_records(&mut app, KeyCode::Down, "navigate_row", "Down");
    assert_records(&mut app, KeyCode::Up, "navigate_row", "Up");
}

#[test]
fn row_first_and_last_jumps_record() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    assert_records(&mut app, KeyCode::Char(']'), "navigate_row_last", "]");
    assert_records(&mut app, KeyCode::Char('['), "navigate_row_first", "[");
    assert_records(&mut app, KeyCode::Char('G'), "navigate_row_last", "G");
}

#[test]
fn lone_g_is_silent_and_the_completed_chord_records_gg() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    // The first `g` only arms the chord — nothing has happened yet.
    assert_silent(&mut app, KeyCode::Char('g'));
    assert_records(&mut app, KeyCode::Char('g'), "navigate_row_first", "gg");
}

#[test]
fn abandoned_chord_records_only_the_key_that_arrived() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.handle_key(make_key(KeyCode::Char('g')));
    // `g` then `j`: the chord is abandoned silently and `j` navigates.
    assert_records(&mut app, KeyCode::Char('j'), "navigate_row", "j");
}

#[test]
fn q_records_quit_on_the_board_and_exit_epic_inside_one() {
    let mut app = make_app();
    assert_records(&mut app, KeyCode::Char('q'), "quit", "q");

    let mut app = epic_app();
    assert_records(&mut app, KeyCode::Char('q'), "exit_epic", "q");
}

#[test]
fn esc_records_only_when_it_has_something_to_clear() {
    // Nothing selected, no search, board view — Esc does nothing.
    let mut app = make_app();
    assert_silent(&mut app, KeyCode::Esc);

    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.handle_key(make_key(KeyCode::Char('v')));
    assert_records(&mut app, KeyCode::Esc, "clear_selection", "Esc");

    let mut app = epic_app();
    assert_records(&mut app, KeyCode::Esc, "exit_epic", "Esc");

    let mut app = make_app();
    app.search.query = "task".to_string();
    assert_records(&mut app, KeyCode::Esc, "clear_search", "Esc");
}

#[test]
fn unbound_board_key_is_silent() {
    let mut app = make_app();
    assert_silent(&mut app, KeyCode::Char('Z'));
}

// ── Space / Enter branches that were previously silent ───────────────────────

#[test]
fn space_on_an_epic_row_records_entering_it() {
    let mut app = make_app_with_epic_selected();
    assert_records(&mut app, KeyCode::Char(' '), "enter_epic", " ");
}

#[test]
fn space_records_an_activation_that_only_reports_it_cannot_proceed() {
    // Done, no worktree: Space can neither resume nor dispatch, and answers
    // with a status hint. The press still happened and still cost the user a
    // keystroke, so it counts.
    let mut app = App::new(vec![make_unprovisioned_task(1, TaskStatus::Done)]);
    app.selection_mut().set_column(4);
    assert_records(&mut app, KeyCode::Char(' '), "activate_unavailable", " ");
}

#[test]
fn enter_records_clearing_select_all() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    // Navigate up from row 0 onto the select-all toggle, then select.
    app.update(Message::NavigateRow(-1));
    app.update(Message::SelectAllColumn);
    assert_records(&mut app, KeyCode::Enter, "clear_select_all", "Enter");
}

// ── task detail overlay ──────────────────────────────────────────────────────

#[test]
fn task_detail_keys_record() {
    let mut app = detail_app();
    assert_records(&mut app, KeyCode::Char('j'), "scroll_detail", "j");
    assert_records(&mut app, KeyCode::Char('k'), "scroll_detail", "k");
    assert_records(&mut app, KeyCode::Down, "scroll_detail", "Down");
    assert_records(&mut app, KeyCode::Up, "scroll_detail", "Up");
    assert_records(&mut app, KeyCode::Char('z'), "zoom_detail", "z");
    assert_records(&mut app, KeyCode::Char('q'), "close_detail", "q");

    let mut app = detail_app();
    assert_records(&mut app, KeyCode::Esc, "close_detail", "Esc");

    let mut app = detail_app();
    assert_records(&mut app, KeyCode::Enter, "close_detail", "Enter");
}

#[test]
fn unbound_task_detail_key_is_silent() {
    let mut app = detail_app();
    assert_silent(&mut app, KeyCode::Char('w'));
}

// ── archive column ──────────────────────────────────────────────────────────

#[test]
fn archive_column_keys_record() {
    let mut app = archive_app();
    assert_records(&mut app, KeyCode::Char('j'), "archive_navigate_row", "j");
    assert_records(&mut app, KeyCode::Char('k'), "archive_navigate_row", "k");
    assert_records(
        &mut app,
        KeyCode::Char(']'),
        "archive_navigate_row_last",
        "]",
    );
    assert_records(
        &mut app,
        KeyCode::Char('['),
        "archive_navigate_row_first",
        "[",
    );
    assert_records(&mut app, KeyCode::Char('e'), "edit_archived", "e");
    assert_records(&mut app, KeyCode::Char('x'), "delete_archived", "x");

    let mut app = archive_app();
    assert_records(&mut app, KeyCode::Char('q'), "quit", "q");

    let mut app = archive_app();
    assert_records(&mut app, KeyCode::Char('h'), "leave_archive", "h");

    let mut app = archive_app();
    assert_records(&mut app, KeyCode::Esc, "leave_archive", "Esc");
}

#[test]
fn archive_actions_on_an_empty_archive_are_silent() {
    let mut app = make_app(); // no archived tasks
    app.selection_mut().set_column(5);
    assert_silent(&mut app, KeyCode::Char('x'));
    assert_silent(&mut app, KeyCode::Char('e'));
    assert_silent(&mut app, KeyCode::Char('j'));
}

// ── TODO overlay: all twelve in-overlay actions ─────────────────────────────

#[test]
fn todo_overlay_navigation_and_list_actions_record() {
    let mut app = todos_app();
    assert_records(&mut app, KeyCode::Char('j'), "todo_move_selection", "j");
    assert_records(&mut app, KeyCode::Char('k'), "todo_move_selection", "k");
    assert_records(&mut app, KeyCode::Down, "todo_move_selection", "Down");
    assert_records(&mut app, KeyCode::Up, "todo_move_selection", "Up");
    assert_records(&mut app, KeyCode::Char('a'), "todo_add", "a");
}

#[test]
fn todo_overlay_item_actions_record() {
    let mut app = todos_app();
    assert_records(&mut app, KeyCode::Char('e'), "todo_edit", "e");

    let mut app = todos_app();
    assert_records(&mut app, KeyCode::Char(' '), "todo_toggle_done", " ");

    let mut app = todos_app();
    assert_records(&mut app, KeyCode::Char('J'), "todo_reorder", "J");
    assert_records(&mut app, KeyCode::Char('K'), "todo_reorder", "K");
    assert_records(&mut app, KeyCode::Char('c'), "todo_clear_done", "c");

    let mut app = todos_app();
    assert_records(&mut app, KeyCode::Char('d'), "todo_delete_prompt", "d");

    let mut app = todos_app();
    assert_records(&mut app, KeyCode::Char('L'), "todo_link_to_task", "L");

    let mut app = todos_app();
    assert_records(&mut app, KeyCode::Tab, "todo_nest", "Tab");

    let mut app = todos_app();
    assert_records(&mut app, KeyCode::BackTab, "todo_unnest", "BackTab");
}

#[test]
fn todo_overlay_close_records() {
    let mut app = todos_app();
    assert_records(&mut app, KeyCode::Char('q'), "close_todos", "q");

    let mut app = todos_app();
    assert_records(&mut app, KeyCode::Esc, "close_todos", "Esc");
}

#[test]
fn todo_unlink_and_jump_record_only_for_a_linked_todo() {
    let mut app = linked_todos_app();
    assert_records(&mut app, KeyCode::Char('U'), "todo_unlink", "U");

    let mut app = linked_todos_app();
    assert_records(&mut app, KeyCode::Enter, "todo_jump_to_linked", "Enter");

    let mut app = linked_todos_app();
    assert_records(&mut app, KeyCode::Char('g'), "todo_jump_to_linked", "g");

    // An unlinked todo has nothing to unlink or jump to.
    let mut app = todos_app();
    assert_silent(&mut app, KeyCode::Char('U'));
    assert_silent(&mut app, KeyCode::Enter);
}

#[test]
fn todo_item_actions_on_an_empty_list_are_silent() {
    for code in [
        KeyCode::Char('e'),
        KeyCode::Char(' '),
        KeyCode::Char('d'),
        KeyCode::Char('L'),
        KeyCode::Char('U'),
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::BackTab,
    ] {
        let mut app = empty_todos_app();
        assert_silent(&mut app, code);
    }
}

// ── confirmation dialogs ────────────────────────────────────────────────────

#[test]
fn shared_confirm_dialogs_record_a_yes_no_pair() {
    // (mode, action prefix)
    let cases: Vec<(InputMode, &str)> = vec![
        (InputMode::ConfirmQuit, "confirm_quit"),
        (InputMode::ConfirmDelete, "confirm_delete"),
        (InputMode::ConfirmArchive(None), "confirm_archive"),
        (InputMode::ConfirmDeleteEpic, "confirm_delete_epic"),
        (InputMode::ConfirmArchiveEpic, "confirm_archive_epic"),
        (
            InputMode::ConfirmDetachTmux(vec![TaskId(1)]),
            "confirm_detach_tmux",
        ),
        (
            InputMode::ConfirmRepoSync {
                repo_path: "/repo".to_string(),
            },
            "confirm_repo_sync",
        ),
    ];
    for (mode, prefix) in cases {
        let mut app = app_in_mode(mode.clone());
        assert_records(&mut app, KeyCode::Char('y'), &format!("{prefix}_yes"), "y");

        let mut app = app_in_mode(mode.clone());
        assert_records(&mut app, KeyCode::Char('n'), &format!("{prefix}_no"), "n");

        let mut app = app_in_mode(mode);
        assert_records(&mut app, KeyCode::Esc, &format!("{prefix}_no"), "Esc");
    }
}

#[test]
fn confirm_done_records_a_yes_no_pair() {
    let mut app = app_in_mode(InputMode::ConfirmDone);
    assert_records(&mut app, KeyCode::Char('y'), "confirm_done_yes", "y");

    let mut app = app_in_mode(InputMode::ConfirmDone);
    assert_records(&mut app, KeyCode::Char('n'), "confirm_done_no", "n");
}

#[test]
fn confirm_retry_records_each_choice() {
    let mut app = app_in_mode(InputMode::ConfirmRetry(TaskId(3)));
    assert_records(&mut app, KeyCode::Char('r'), "confirm_retry_resume", "r");

    let mut app = app_in_mode(InputMode::ConfirmRetry(TaskId(3)));
    assert_records(&mut app, KeyCode::Char('f'), "confirm_retry_fresh", "f");

    let mut app = app_in_mode(InputMode::ConfirmRetry(TaskId(3)));
    assert_records(&mut app, KeyCode::Esc, "confirm_retry_no", "Esc");

    // The retry dialog ignores anything else — no event.
    let mut app = app_in_mode(InputMode::ConfirmRetry(TaskId(3)));
    assert_silent(&mut app, KeyCode::Char('z'));
}

#[test]
fn confirm_delete_todo_records_a_yes_no_pair() {
    let mut app = todos_app();
    app.handle_key(make_key(KeyCode::Char('d')));
    assert_records(&mut app, KeyCode::Char('y'), "confirm_delete_todo_yes", "y");

    let mut app = todos_app();
    app.handle_key(make_key(KeyCode::Char('d')));
    assert_records(&mut app, KeyCode::Char('n'), "confirm_delete_todo_no", "n");

    let mut app = todos_app();
    app.handle_key(make_key(KeyCode::Char('d')));
    assert_silent(&mut app, KeyCode::Char('z'));
}

#[test]
fn confirm_trust_repo_records_a_yes_no_pair() {
    let mut app = app_in_mode(InputMode::ConfirmTrustRepo {
        task_id: TaskId(1),
        mode: crate::models::DispatchMode::Dispatch,
    });
    assert_records(&mut app, KeyCode::Char('y'), "confirm_trust_repo_yes", "y");

    let mut app = app_in_mode(InputMode::ConfirmTrustRepo {
        task_id: TaskId(1),
        mode: crate::models::DispatchMode::Dispatch,
    });
    assert_records(&mut app, KeyCode::Char('n'), "confirm_trust_repo_no", "n");
}

#[test]
fn confirm_trust_repo_quick_dispatch_records_a_yes_no_pair() {
    let draft = TaskDraft::default();
    let mut app = app_in_mode(InputMode::ConfirmTrustRepoQuickDispatch {
        draft: draft.clone(),
        epic_id: None,
    });
    assert_records(
        &mut app,
        KeyCode::Char('y'),
        "confirm_trust_repo_quick_dispatch_yes",
        "y",
    );

    let mut app = app_in_mode(InputMode::ConfirmTrustRepoQuickDispatch {
        draft,
        epic_id: None,
    });
    assert_records(
        &mut app,
        KeyCode::Char('n'),
        "confirm_trust_repo_quick_dispatch_no",
        "n",
    );
}

// ── pickers ─────────────────────────────────────────────────────────────────

#[test]
fn search_mode_records_commit_and_cancel_but_not_typing() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('/')));
    assert_silent(&mut app, KeyCode::Char('t'));
    assert_silent(&mut app, KeyCode::Backspace);
    assert_records(&mut app, KeyCode::Enter, "search_commit", "Enter");

    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('/')));
    assert_records(&mut app, KeyCode::Esc, "search_cancel", "Esc");
}

#[test]
fn tag_picker_records_selection_default_and_cancel() {
    let mut app = app_in_mode(InputMode::InputTag);
    assert_records(&mut app, KeyCode::Char('b'), "tag_picker_select", "b");

    let mut app = app_in_mode(InputMode::InputTag);
    assert_records(&mut app, KeyCode::Enter, "tag_picker_default", "Enter");

    let mut app = app_in_mode(InputMode::InputTag);
    assert_records(&mut app, KeyCode::Esc, "tag_picker_cancel", "Esc");

    // A character that maps to no tag is ignored.
    let mut app = app_in_mode(InputMode::InputTag);
    assert_silent(&mut app, KeyCode::Char('z'));
}

#[test]
fn wrap_up_mode_picker_records_selection_default_and_cancel() {
    let mut app = app_in_mode(InputMode::InputWrapUpMode);
    assert_records(
        &mut app,
        KeyCode::Char('r'),
        "wrap_up_mode_picker_select",
        "r",
    );

    let mut app = app_in_mode(InputMode::InputWrapUpMode);
    assert_records(
        &mut app,
        KeyCode::Enter,
        "wrap_up_mode_picker_default",
        "Enter",
    );

    let mut app = app_in_mode(InputMode::InputWrapUpMode);
    assert_records(&mut app, KeyCode::Esc, "wrap_up_mode_picker_cancel", "Esc");
}

#[test]
fn quick_dispatch_picker_records_navigation_select_and_cancel() {
    let mut app = app_in_mode(InputMode::QuickDispatch);
    assert_records(
        &mut app,
        KeyCode::Down,
        "quick_dispatch_move_cursor",
        "Down",
    );
    assert_records(&mut app, KeyCode::Up, "quick_dispatch_move_cursor", "Up");
    assert_silent(&mut app, KeyCode::Char('r')); // filtering, not an action
    assert_records(&mut app, KeyCode::Enter, "quick_dispatch_select", "Enter");

    let mut app = app_in_mode(InputMode::QuickDispatch);
    assert_records(&mut app, KeyCode::Esc, "quick_dispatch_cancel", "Esc");
}

#[test]
fn text_modes_record_commit_and_cancel_but_not_typing() {
    // Typing a title is data entry, not a use of a keybinding.
    let mut app = app_in_mode(InputMode::InputTitle);
    assert_silent(&mut app, KeyCode::Char('t'));
    assert_silent(&mut app, KeyCode::Backspace);
    assert_records(&mut app, KeyCode::Enter, "submit_input", "Enter");

    let mut app = app_in_mode(InputMode::InputTitle);
    assert_records(&mut app, KeyCode::Esc, "cancel_input", "Esc");
}

#[test]
fn repo_path_picker_records_cursor_moves() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    assert_records(&mut app, KeyCode::Down, "picker_move_cursor", "Down");
    assert_records(&mut app, KeyCode::Up, "picker_move_cursor", "Up");
}

#[test]
fn help_overlay_close_records() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('?')));
    assert_records(&mut app, KeyCode::Esc, "close_help", "Esc");

    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('?')));
    assert_records(&mut app, KeyCode::Char('?'), "close_help", "?");
}

#[test]
fn repo_filter_toggles_and_presets_record() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.handle_key(make_key(KeyCode::Char('f')));
    assert_records(&mut app, KeyCode::Char('j'), "repo_filter_move_cursor", "j");
    assert_records(&mut app, KeyCode::Char('k'), "repo_filter_move_cursor", "k");
    assert_records(
        &mut app,
        KeyCode::Char(' '),
        "repo_filter_toggle_only_active",
        " ",
    );
    assert_records(&mut app, KeyCode::Char('a'), "repo_filter_toggle_all", "a");
    assert_records(&mut app, KeyCode::Tab, "repo_filter_toggle_mode", "Tab");
    assert_records(&mut app, KeyCode::Char('1'), "repo_filter_toggle_repo", "1");
    assert_records(&mut app, KeyCode::Char('s'), "repo_filter_save_preset", "s");

    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.handle_key(make_key(KeyCode::Char('f')));
    assert_records(
        &mut app,
        KeyCode::Char('x'),
        "repo_filter_delete_preset",
        "x",
    );

    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.handle_key(make_key(KeyCode::Char('f')));
    assert_records(&mut app, KeyCode::Char('q'), "repo_filter_close", "q");
}

#[test]
fn repo_filter_out_of_range_selections_are_silent() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.handle_key(make_key(KeyCode::Char('f')));
    assert_silent(&mut app, KeyCode::Char('9')); // no 9th repo
    assert_silent(&mut app, KeyCode::Char('Z')); // no preset Z
    assert_silent(&mut app, KeyCode::Backspace); // cursor is on "only active"
}

#[test]
fn reparent_picker_records_navigation_confirm_and_cancel() {
    let mut app = make_app_with_epic_selected();
    app.handle_key(make_key(KeyCode::Char('m')));
    assert_records(
        &mut app,
        KeyCode::Char('j'),
        "reparent_picker_navigate",
        "j",
    );
    assert_records(&mut app, KeyCode::Enter, "reparent_picker_confirm", "Enter");

    let mut app = make_app_with_epic_selected();
    app.handle_key(make_key(KeyCode::Char('m')));
    assert_records(&mut app, KeyCode::Esc, "reparent_picker_cancel", "Esc");
}

#[test]
fn move_to_epic_picker_records_navigation_confirm_and_cancel() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.handle_key(make_key(KeyCode::Char('m')));
    assert_records(
        &mut app,
        KeyCode::Char('j'),
        "move_to_epic_picker_navigate",
        "j",
    );
    assert_records(
        &mut app,
        KeyCode::Enter,
        "move_to_epic_picker_confirm",
        "Enter",
    );

    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.handle_key(make_key(KeyCode::Char('m')));
    assert_records(&mut app, KeyCode::Esc, "move_to_epic_picker_cancel", "Esc");
}

#[test]
fn link_todo_picker_records_navigation_confirm_and_cancel() {
    let mut app = linked_todos_app();
    app.handle_key(make_key(KeyCode::Char('L')));
    app.selection_mut().set_column(1);
    assert_records(&mut app, KeyCode::Char('j'), "link_todo_navigate", "j");
    assert_records(&mut app, KeyCode::Enter, "link_todo_confirm", "Enter");

    let mut app = linked_todos_app();
    app.handle_key(make_key(KeyCode::Char('L')));
    assert_records(&mut app, KeyCode::Esc, "link_todo_cancel", "Esc");
}

#[test]
fn error_popup_dismissal_records() {
    let mut app = make_app();
    app.status.error_popup = Some("boom".to_string());
    assert_records(&mut app, KeyCode::Enter, "dismiss_error", "Enter");
}
