use super::*;
use crossterm::event::KeyCode;

// ---------------------------------------------------------------------------
// `I` / learnings overlay removal (docs/plans/3809-keybinding-pruning-implementation.md
// §3): the TUI knowledge-base browsing/curation surface is gone. Learnings are
// curated exclusively via MCP (`record_learning`, `rate_learning`,
// `query_learnings`, `delete_learning`); the background stale-learning sweep
// (`LearningCommand::ArchiveStale`) is unaffected and is tested separately in
// `src/runtime/tests.rs`.
// ---------------------------------------------------------------------------

#[test]
fn i_key_is_inert_on_a_fresh_board() {
    let mut app = make_app();
    let mode_before = app.input.mode.clone();

    let cmds = app.handle_key(make_key(KeyCode::Char('I')));

    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, mode_before);
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn action_hints_no_longer_advertises_the_learnings_key() {
    let hints = crate::tui::ui::action_hints(None, false, ratatui::style::Color::White);
    let joined: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        !joined.to_lowercase().contains("learnings"),
        "action_hints should no longer advertise the retired [I] learnings key, got: {joined:?}"
    );
}
