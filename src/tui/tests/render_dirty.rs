//! Tests for dirty-flag correctness: handle_key must always set dirty=true
//! after processing a key, including for no-op navigation (cursor at a
//! boundary). Earlier revisions tried to skip the redraw for such no-ops by
//! snapshotting which fields changed, but that opt-in snapshot proved fragile
//! — several popup/overlay handlers mutated state invisible to the snapshot
//! and forgot to set dirty themselves, causing dropped frames (keystrokes
//! with no visible effect until an unrelated event happened to redraw). See
//! the dirty-flag section of docs/architecture.md. The 16ms frame-rate cap in
//! `frame_ready` already bounds the redraw cost, so always marking dirty is
//! both correct and cheap.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crossterm::event::KeyCode;

// ---------------------------------------------------------------------------
// Dirty signal: no-op navigation still marks dirty
// ---------------------------------------------------------------------------

#[test]
fn noop_nav_at_row_boundary_still_sets_dirty() {
    let mut app = make_app(); // 2 tasks in Backlog (col 1)
                              // Move to last row (row index 1 = second task).
    app.update(Message::NavigateRow(1));
    app.dirty = false;

    // j at the last row is a no-op — cursor doesn't move — but handle_key
    // still marks the frame dirty unconditionally.
    app.handle_key(make_key(KeyCode::Char('j')));

    assert!(
        app.dirty,
        "handle_key must set dirty even for no-op navigation; got dirty=false"
    );
}

#[test]
fn noop_nav_at_col_boundary_still_sets_dirty() {
    let mut app = make_app(); // starts in Backlog (leftmost task column)
    app.dirty = false;

    // h at the leftmost column stays in the same column, but still dirty.
    app.handle_key(make_key(KeyCode::Char('h')));

    assert!(
        app.dirty,
        "handle_key must set dirty even for no-op navigation; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: navigation that actually moves the cursor
// ---------------------------------------------------------------------------

#[test]
fn nav_that_moves_row_sets_dirty() {
    let mut app = make_app(); // 2 tasks in Backlog, cursor at row 0
    app.dirty = false;

    // j moves from row 0 to row 1 — a real state change.
    app.handle_key(make_key(KeyCode::Char('j')));

    assert!(
        app.dirty,
        "pressing j when cursor can move must set dirty; got dirty=false"
    );
}

#[test]
fn nav_that_moves_column_sets_dirty() {
    let mut app = make_app(); // starts in Backlog (col 1)
    app.dirty = false;

    // l moves to Running (col 2) — a real state change.
    app.handle_key(make_key(KeyCode::Char('l')));

    assert!(
        app.dirty,
        "pressing l when cursor can move right must set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: non-navigation keys always set dirty
// ---------------------------------------------------------------------------

#[test]
fn non_nav_key_sets_dirty() {
    let mut app = make_app();
    app.dirty = false;

    // 'n' opens the new-task input — always a state change.
    app.handle_key(make_key(KeyCode::Char('n')));

    assert!(
        app.dirty,
        "pressing 'n' (open new task) must set dirty; got dirty=false"
    );
}

#[test]
fn noop_nav_via_down_arrow_still_sets_dirty() {
    let mut app = make_app();
    app.update(Message::NavigateRow(1)); // move to last row
    app.dirty = false;

    app.handle_key(make_key(KeyCode::Down));

    assert!(
        app.dirty,
        "Down arrow at last row must still set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: todo popup navigation
// ---------------------------------------------------------------------------

#[test]
fn todo_selection_move_sets_dirty() {
    let mut app = make_app();
    // Open todos with two items so j can actually move.
    app.update(Message::Todo(crate::tui::messages::TodoMessage::Show(
        vec![make_todo(1, "first"), make_todo(2, "second")],
    )));
    app.dirty = false;

    // j moves selection from 0 → 1 — a real state change.
    app.handle_key(make_key(KeyCode::Char('j')));

    assert!(
        app.dirty,
        "pressing j in the todo popup when cursor can move must set dirty; got dirty=false"
    );
}

#[test]
fn todo_selection_at_boundary_still_sets_dirty() {
    let mut app = make_app();
    // Single item — j is a no-op (already at last row), but still dirty.
    app.update(Message::Todo(crate::tui::messages::TodoMessage::Show(
        vec![make_todo(1, "only")],
    )));
    app.dirty = false;

    app.handle_key(make_key(KeyCode::Char('j')));

    assert!(
        app.dirty,
        "pressing j at the last todo row must still set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: nested epic navigation
// ---------------------------------------------------------------------------

#[test]
fn entering_nested_epic_sets_dirty() {
    use crate::models::EpicId;
    use crate::tui::messages::EpicMessage;
    use crate::tui::types::{BoardSelection, ViewMode};

    let mut app = make_app();
    let mut child_epic = make_epic(20);
    child_epic.parent_epic_id = Some(EpicId(10));
    app.board.epics = vec![make_epic(10), child_epic];
    // Start inside epic 10
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    app.dirty = false;

    // Enter the nested sub-epic 20 — same ViewMode discriminant, different epic_id
    app.update(Message::Epic(EpicMessage::Enter(EpicId(20))));

    assert!(
        app.dirty,
        "entering a nested epic must set dirty; got dirty=false"
    );
}

#[test]
fn exiting_nested_epic_sets_dirty() {
    use crate::models::EpicId;
    use crate::tui::messages::EpicMessage;
    use crate::tui::types::{BoardSelection, ViewMode};

    let mut app = make_app();
    let mut child_epic = make_epic(20);
    child_epic.parent_epic_id = Some(EpicId(10));
    app.board.epics = vec![make_epic(10), child_epic];
    // Start inside nested epic 20, parent is epic 10
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(20),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Epic {
            epic_id: EpicId(10),
            selection: BoardSelection::new_for_epic(),
            parent: Box::new(ViewMode::Board(BoardSelection::new())),
        }),
    };
    app.dirty = false;

    // Exit back to parent epic 10 — same ViewMode discriminant, different epic_id
    app.update(Message::Epic(EpicMessage::Exit));

    assert!(
        app.dirty,
        "exiting a nested epic must set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: reparent epic popup navigation
// ---------------------------------------------------------------------------

#[test]
fn reparent_navigate_down_sets_dirty() {
    use crate::models::EpicId;
    use crate::tui::messages::EpicMessage;
    use crate::tui::types::TreeNav;

    let mut app = make_app();
    app.board.epics = vec![make_epic(10), make_epic(20)];
    // Open the reparent picker via the handler so state is properly initialized.
    app.update(Message::Epic(EpicMessage::StartReparent(EpicId(10))));
    app.dirty = false;

    app.update(Message::Epic(EpicMessage::ReparentNavigate(TreeNav::Down)));

    assert!(
        app.dirty,
        "navigating down in the reparent picker must set dirty; got dirty=false"
    );
}

#[test]
fn reparent_navigate_up_sets_dirty() {
    use crate::models::EpicId;
    use crate::tui::messages::EpicMessage;
    use crate::tui::types::TreeNav;

    let mut app = make_app();
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.update(Message::Epic(EpicMessage::StartReparent(EpicId(10))));
    // Navigate down first so up can actually move.
    app.update(Message::Epic(EpicMessage::ReparentNavigate(TreeNav::Down)));
    app.dirty = false;

    app.update(Message::Epic(EpicMessage::ReparentNavigate(TreeNav::Up)));

    assert!(
        app.dirty,
        "navigating up in the reparent picker must set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: move-task-to-epic popup navigation
// ---------------------------------------------------------------------------

#[test]
fn move_to_epic_navigate_down_sets_dirty() {
    use crate::models::{TaskId, TaskStatus};
    use crate::tui::messages::TaskMessage;
    use crate::tui::types::TreeNav;

    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.update(Message::Task(TaskMessage::StartMoveToEpic(TaskId(1))));
    app.dirty = false;

    app.update(Message::Task(TaskMessage::MoveToEpicNavigate(
        TreeNav::Down,
    )));

    assert!(
        app.dirty,
        "navigating down in the move-task picker must set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: repo filter popup cursor navigation
// ---------------------------------------------------------------------------

#[test]
fn repo_filter_cursor_move_sets_dirty() {
    use crate::tui::messages::RepoFilterMessage;

    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.dirty = false;

    app.update(Message::RepoFilter(RepoFilterMessage::MoveCursor(1)));

    assert!(
        app.dirty,
        "moving cursor down in the repo filter popup must set dirty; got dirty=false"
    );
}

#[test]
fn repo_filter_cursor_move_via_key_sets_dirty() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    // Enter repo filter mode.
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Start,
    ));
    app.dirty = false;

    app.handle_key(make_key(KeyCode::Char('j')));

    assert!(
        app.dirty,
        "pressing j in the repo filter popup must set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: repo filter popup toggle operations
// ---------------------------------------------------------------------------

#[test]
fn repo_filter_toggle_sets_dirty() {
    use crate::tui::messages::RepoFilterMessage;

    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.dirty = false;

    app.update(Message::RepoFilter(RepoFilterMessage::Toggle(
        "/repo-a".to_string(),
    )));

    assert!(
        app.dirty,
        "toggling a repo filter must set dirty; got dirty=false"
    );
}

#[test]
fn repo_filter_toggle_via_space_key_sets_dirty() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Start,
    ));
    // Move cursor to first repo entry (cursor 1 = first repo_path)
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::MoveCursor(1),
    ));
    app.dirty = false;

    app.handle_key(make_key(KeyCode::Char(' ')));

    assert!(
        app.dirty,
        "pressing space in the repo filter popup must set dirty; got dirty=false"
    );
}

#[test]
fn repo_filter_toggle_only_active_sets_dirty() {
    use crate::tui::messages::RepoFilterMessage;

    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.dirty = false;

    app.update(Message::RepoFilter(RepoFilterMessage::ToggleOnlyActive));

    assert!(
        app.dirty,
        "toggling only-active must set dirty; got dirty=false"
    );
}

#[test]
fn repo_filter_toggle_all_sets_dirty() {
    use crate::tui::messages::RepoFilterMessage;

    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.dirty = false;

    app.update(Message::RepoFilter(RepoFilterMessage::ToggleAll));

    assert!(
        app.dirty,
        "toggling all repos must set dirty; got dirty=false"
    );
}

#[test]
fn repo_filter_toggle_mode_sets_dirty() {
    use crate::tui::messages::RepoFilterMessage;

    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.dirty = false;

    app.update(Message::RepoFilter(RepoFilterMessage::ToggleMode));

    assert!(
        app.dirty,
        "toggling filter mode must set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: learnings overlay navigation
// ---------------------------------------------------------------------------

#[test]
fn learnings_selection_move_sets_dirty() {
    use crate::models::{Learning, LearningId, LearningKind, LearningScope, LearningStatus};
    use crate::tui::messages::LearningMessage;
    let mut app = make_app();
    let now = chrono::Utc::now();
    let make_learning = |id: i64| Learning {
        id: LearningId(id),
        kind: LearningKind::Convention,
        summary: format!("learning {id}"),
        detail: None,
        scope: LearningScope::User,
        scope_ref: None,
        tags: vec![],
        status: LearningStatus::Approved,
        source_task_id: None,
        upvote_count: 0,
        last_upvoted_at: None,
        created_at: now,
        updated_at: now,
    };
    app.update(Message::Learning(LearningMessage::Show(vec![
        make_learning(1),
        make_learning(2),
    ])));
    app.dirty = false;

    app.handle_key(make_key(KeyCode::Char('j')));

    assert!(
        app.dirty,
        "pressing j in the learnings overlay when cursor can move must set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: learnings overlay Tree-view navigation (tree_state RefCell)
// ---------------------------------------------------------------------------

#[test]
fn learnings_navigate_tree_sets_dirty() {
    use super::learning_review::make_learning;
    use crate::models::LearningId;
    use crate::tui::messages::LearningMessage;
    use crate::tui::types::TreeNav;

    let mut app = make_app();
    app.update(Message::Learning(LearningMessage::Show(vec![
        make_learning(LearningId(1)),
        make_learning(LearningId(2)),
    ])));
    app.update(Message::Learning(LearningMessage::ToggleView));
    app.dirty = false;

    app.update(Message::Learning(LearningMessage::NavigateTree(
        TreeNav::Down,
    )));

    assert!(
        app.dirty,
        "navigating the learnings tree view must set dirty; got dirty=false"
    );
}

// Regression tests for the KnowledgeBaseReview `NavigationAlwaysRedraws`
// guarantee (docs/specs/learnings.allium): every Navigate / ToggleView /
// CollapseGroup / ExpandGroup must redraw when driven through the real keypress
// path. The tree cursor and open-state live in a RefCell<TreeState> that the
// central handle_key change-detector cannot observe, so these paths must force
// dirty themselves — a handler that mutates only tree state without flagging
// dirty would appear frozen.

/// Helper: enter the learnings overlay in Tree view with two entries.
fn app_in_learnings_tree() -> crate::tui::App {
    use super::learning_review::make_learning;
    use crate::models::LearningId;
    use crate::tui::messages::LearningMessage;
    let mut app = make_app();
    app.update(Message::Learning(LearningMessage::Show(vec![
        make_learning(LearningId(1)),
        make_learning(LearningId(2)),
    ])));
    app.update(Message::Learning(LearningMessage::ToggleView));
    app
}

#[test]
fn learnings_tree_j_key_sets_dirty() {
    let mut app = app_in_learnings_tree();
    app.dirty = false;
    app.handle_key(make_key(KeyCode::Char('j')));
    assert!(
        app.dirty,
        "pressing j in the learnings tree view must set dirty; got dirty=false"
    );
}

#[test]
fn learnings_tree_collapse_expand_keys_set_dirty() {
    let mut app = app_in_learnings_tree();

    app.dirty = false;
    app.handle_key(make_key(KeyCode::Char('h')));
    assert!(
        app.dirty,
        "collapsing a group (h) in the learnings tree view must set dirty; \
         open-state changes are invisible to the selection snapshot"
    );

    app.dirty = false;
    app.handle_key(make_key(KeyCode::Char('l')));
    assert!(
        app.dirty,
        "expanding a group (l) in the learnings tree view must set dirty"
    );
}

#[test]
fn learnings_toggle_view_key_sets_dirty() {
    use super::learning_review::make_learning;
    use crate::models::LearningId;
    use crate::tui::messages::LearningMessage;
    let mut app = make_app();
    app.update(Message::Learning(LearningMessage::Show(vec![
        make_learning(LearningId(1)),
    ])));
    // Starts in List view; Tab toggles to Tree. The ViewMode discriminant is
    // unchanged (still Learnings) and `selected` does not move, so this only
    // redraws if the toggle path forces dirty.
    app.dirty = false;
    app.handle_key(make_key(KeyCode::Tab));
    assert!(
        app.dirty,
        "toggling the learnings view (Tab) must set dirty; got dirty=false"
    );
    assert!(
        matches!(
            &app.board.view_mode,
            crate::tui::types::ViewMode::Learnings {
                view: crate::tui::types::LearningsView::Tree,
                ..
            }
        ),
        "Tab must switch List -> Tree"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: repo filter preset load (mutates filter.repos/mode, invisible
// to the handle_key snapshot)
// ---------------------------------------------------------------------------

#[test]
fn load_filter_preset_sets_dirty() {
    use crate::tui::messages::RepoFilterMessage;
    use std::collections::HashSet;

    let mut app = make_app();
    app.board.repo_paths = vec!["/repo/a".to_string()];
    app.filter.presets = vec![(
        "my-preset".to_string(),
        HashSet::from(["/repo/a".to_string()]),
        RepoFilterMode::Include,
    )];
    app.input.mode = InputMode::RepoFilter;
    app.dirty = false;

    app.update(Message::RepoFilter(RepoFilterMessage::LoadPreset(
        "my-preset".to_string(),
    )));

    assert!(
        app.dirty,
        "loading a filter preset must set dirty; got dirty=false"
    );
}

// ---------------------------------------------------------------------------
// Dirty signal: managed feed config popup (typing/tabbing mutates
// self.interaction.managed_feed_config, entirely invisible to the handle_key snapshot)
// ---------------------------------------------------------------------------

#[test]
fn managed_feed_config_input_sets_dirty() {
    use crate::tui::messages::ManagedFeedConfigMessage;

    let mut app = make_app();
    app.update(Message::ManagedFeedConfig(ManagedFeedConfigMessage::Open));
    app.dirty = false;

    app.update(Message::ManagedFeedConfig(ManagedFeedConfigMessage::Input(
        '5',
    )));

    assert!(
        app.dirty,
        "typing in the managed feed config popup must set dirty; got dirty=false"
    );
}

#[test]
fn managed_feed_config_move_field_sets_dirty() {
    use crate::tui::messages::ManagedFeedConfigMessage;

    let mut app = make_app();
    app.update(Message::ManagedFeedConfig(ManagedFeedConfigMessage::Open));
    app.dirty = false;

    app.update(Message::ManagedFeedConfig(
        ManagedFeedConfigMessage::MoveField(1),
    ));

    assert!(
        app.dirty,
        "moving the field cursor in the managed feed config popup must set dirty; got dirty=false"
    );
}

#[test]
fn managed_feed_config_backspace_sets_dirty() {
    use crate::tui::messages::ManagedFeedConfigMessage;

    let mut app = make_app();
    app.update(Message::ManagedFeedConfig(ManagedFeedConfigMessage::Open));
    app.update(Message::ManagedFeedConfig(ManagedFeedConfigMessage::Input(
        '5',
    )));
    app.dirty = false;

    app.update(Message::ManagedFeedConfig(
        ManagedFeedConfigMessage::Backspace,
    ));

    assert!(
        app.dirty,
        "backspacing in the managed feed config popup must set dirty; got dirty=false"
    );
}
