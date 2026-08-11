#![allow(clippy::unwrap_used, clippy::expect_used)]
use crossterm::event::KeyCode;

use super::*;

fn make_app() -> App {
    App::new(vec![])
}

// ── keybinding ──

#[test]
fn colon_emits_open_when_dir_unset() {
    // `:` always delegates to the runtime, which decides whether to jump to a
    // live window or open the picker — it no longer enters the picker directly.
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char(':')));
    assert_eq!(app.mode(), &InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::MainSession(crate::tui::commands::MainSessionCommand::Open)
    )));
}

#[test]
fn colon_emits_open_when_dir_set() {
    let mut app = make_app();
    app.set_main_session_dir(Some("/home/user".to_string()));
    let cmds = app.handle_key(make_key(KeyCode::Char(':')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::MainSession(crate::tui::commands::MainSessionCommand::Open)
    )));
}

#[test]
fn configure_message_enters_main_session_dir_mode() {
    let mut app = make_app();
    app.update(Message::MainSession(
        crate::tui::messages::MainSessionMessage::Configure,
    ));
    assert_eq!(app.mode(), &InputMode::MainSessionDir);
}

#[test]
fn full_reconfigure_flow_open_to_create() {
    // `:` → Open; runtime (no live window) feeds Configure → picker; typing a
    // path + Enter → persist dir + Create. Exercises the whole sequence at the
    // App level (the live-window check itself lives in the runtime).
    let mut app = make_app();

    let cmds = app.handle_key(make_key(KeyCode::Char(':')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::MainSession(crate::tui::commands::MainSessionCommand::Open)
    )));

    app.update(Message::MainSession(
        crate::tui::messages::MainSessionMessage::Configure,
    ));
    assert_eq!(app.mode(), &InputMode::MainSessionDir);

    app.input.set_buffer("/home/user/code".to_string());
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert!(cmds.iter().any(
        |c| matches!(c, Command::PersistStringSetting { key, .. } if key == "main_session.dir")
    ));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::MainSession(crate::tui::commands::MainSessionCommand::Create)
    )));
    assert_eq!(app.mode(), &InputMode::Normal);
}

// ── text input in MainSessionDir mode ──

#[test]
fn typing_in_main_session_dir_mode_accumulates_in_buffer() {
    let mut app = make_app();
    app.input.mode = InputMode::MainSessionDir;
    app.handle_key(make_key(KeyCode::Char('/')));
    app.handle_key(make_key(KeyCode::Char('h')));
    app.handle_key(make_key(KeyCode::Char('o')));
    assert_eq!(app.input_buffer(), "/ho");
}

#[test]
fn enter_in_main_session_dir_mode_emits_submit_message() {
    let mut app = make_app();
    app.input.mode = InputMode::MainSessionDir;
    app.input.set_buffer("/home/user".to_string());
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert!(cmds.iter().any(
        |c| matches!(c, Command::PersistStringSetting { key, .. } if key == "main_session.dir")
    ));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::MainSession(crate::tui::commands::MainSessionCommand::Create)
    )));
}

#[test]
fn enter_with_empty_buffer_cancels_main_session_dir() {
    let mut app = make_app();
    app.input.mode = InputMode::MainSessionDir;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    assert!(cmds.is_empty());
    assert_eq!(app.mode(), &InputMode::Normal);
}

#[test]
fn esc_in_main_session_dir_mode_returns_to_normal() {
    let mut app = make_app();
    app.input.mode = InputMode::MainSessionDir;
    app.input.set_buffer("/some/path".to_string());
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.mode(), &InputMode::Normal);
    assert_eq!(app.input_buffer(), "");
}

// ── message handlers ──

#[test]
fn submit_main_session_dir_sets_dir_on_app() {
    let mut app = make_app();
    app.update(Message::MainSession(
        crate::tui::messages::MainSessionMessage::SubmitDir("/home/user".to_string()),
    ));
    assert_eq!(app.main_session_dir(), Some("/home/user"));
}

#[test]
fn submit_main_session_dir_expands_tilde() {
    let mut app = make_app();
    app.update(Message::MainSession(
        crate::tui::messages::MainSessionMessage::SubmitDir("~/code".to_string()),
    ));
    let dir = app.main_session_dir().unwrap();
    assert!(
        !dir.starts_with('~'),
        "tilde should be expanded, got: {dir}"
    );
}

#[test]
fn submit_main_session_dir_returns_persist_and_create_commands() {
    let mut app = make_app();
    let cmds = app.update(Message::MainSession(
        crate::tui::messages::MainSessionMessage::SubmitDir("/home/user".to_string()),
    ));
    assert!(cmds.iter().any(
        |c| matches!(c, Command::PersistStringSetting { key, .. } if key == "main_session.dir")
    ));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::MainSession(crate::tui::commands::MainSessionCommand::Create)
    )));
}

#[test]
fn submit_main_session_dir_resets_input_mode() {
    let mut app = make_app();
    app.input.mode = InputMode::MainSessionDir;
    app.update(Message::MainSession(
        crate::tui::messages::MainSessionMessage::SubmitDir("/home/user".to_string()),
    ));
    assert_eq!(app.mode(), &InputMode::Normal);
}

// ── fuzzy repo_path history selection (#612) ──

fn make_app_with_repos(repos: &[&str]) -> App {
    let mut app = make_app();
    app.board.repo_paths = repos.iter().map(|s| s.to_string()).collect();
    app
}

#[test]
fn arrow_keys_navigate_filtered_repo_paths_in_main_session_dir() {
    let mut app = make_app_with_repos(&["/a/foo", "/a/bar", "/b/foo"]);
    app.input.mode = InputMode::MainSessionDir;
    // Type "foo" — fuzzy matches "/a/foo" and "/b/foo".
    // "foo" is not an exact match, so there are 3 effective entries:
    // index 0 = "/a/foo", index 1 = "/b/foo", index 2 = new-path "foo".
    app.handle_key(make_key(KeyCode::Char('f')));
    app.handle_key(make_key(KeyCode::Char('o')));
    app.handle_key(make_key(KeyCode::Char('o')));
    assert_eq!(app.input.repo_cursor, 0);

    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 1);

    // Moves to new-path slot (index 2)
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 2);

    // Wraps back to 0
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn enter_with_fuzzy_match_submits_filtered_selection_in_main_session_dir() {
    let mut app = make_app_with_repos(&["/a/foo", "/a/bar", "/b/foo"]);
    app.input.mode = InputMode::MainSessionDir;
    app.handle_key(make_key(KeyCode::Char('b')));
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('r')));
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::PersistStringSetting { key, value }
        if key == "main_session.dir" && value == "/a/bar")),
        "expected persist of /a/bar from filtered match, got: {cmds:?}"
    );
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::MainSession(crate::tui::commands::MainSessionCommand::Create)
    )));
}

#[test]
fn enter_with_no_fuzzy_match_submits_literal_buffer_in_main_session_dir() {
    let mut app = make_app_with_repos(&["/a/foo"]);
    app.input.mode = InputMode::MainSessionDir;
    for c in "/totally/new/path".chars() {
        app.handle_key(make_key(KeyCode::Char(c)));
    }
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::PersistStringSetting { key, value }
        if key == "main_session.dir" && value == "/totally/new/path")),
        "expected literal buffer to be submitted when no history match, got: {cmds:?}"
    );
}

// ── MainSessionIndicator badge (docs/specs/dispatch.allium: MainSessionIndicator) ──

// @guarantee AliveBadgeWhenWindowAlive: alive window → filled dot `● main`.
#[test]
fn badge_shows_alive_dot_when_main_session_alive() {
    let mut app = make_app();
    app.main_session_alive = true;
    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "● main"),
        "expected alive badge `● main` in status bar"
    );
}

// @guarantee NotRunningBadgeWhenConfiguredButDead: configured & !alive → hollow
// dot `○ main`, never the filled dot.
#[test]
fn badge_shows_hollow_dot_when_configured_but_not_alive() {
    let mut app = make_app();
    app.set_main_session_dir(Some("/home/user".to_string()));
    app.main_session_alive = false;
    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "○ main"),
        "expected not-running badge `○ main` in status bar"
    );
    assert!(
        !buffer_contains(&buf, "● main"),
        "must not show the alive badge when the window is not alive"
    );
}

// @guarantee HiddenWhenNeverConfigured: neither alive nor configured → no badge.
#[test]
fn badge_hidden_when_not_configured_and_not_alive() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        !buffer_contains(&buf, "● main") && !buffer_contains(&buf, "○ main"),
        "no main-session badge expected when never configured and not alive"
    );
}

// @guarantee AliveBadgeWhenWindowAlive (edge case): liveness alone drives the
// alive style even when no directory was ever configured.
#[test]
fn badge_shows_alive_dot_even_when_dir_unconfigured() {
    let mut app = make_app();
    app.main_session_alive = true;
    assert_eq!(app.main_session_dir(), None);
    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "● main"),
        "alive window shows the badge even with no configured dir"
    );
}

// @guarantee LivenessRefreshedPeriodically: a refresh that CHANGES window_alive
// marks the board dirty so the badge redraws.
#[test]
fn liveness_changed_marks_dirty() {
    let mut app = make_app();
    app.dirty = false;
    app.update(Message::MainSession(
        crate::tui::messages::MainSessionMessage::LivenessChanged(true),
    ));
    assert!(app.main_session_alive, "field should reflect the new value");
    assert!(app.dirty, "a changed liveness value must trigger a redraw");
}

// @guarantee LivenessRefreshedPeriodically: a no-op refresh (same value) must
// NOT force a redraw.
#[test]
fn liveness_changed_noop_does_not_mark_dirty() {
    let mut app = make_app();
    app.update(Message::MainSession(
        crate::tui::messages::MainSessionMessage::LivenessChanged(true),
    ));
    app.dirty = false;
    app.update(Message::MainSession(
        crate::tui::messages::MainSessionMessage::LivenessChanged(true),
    ));
    assert!(
        !app.dirty,
        "an unchanged liveness value must not force a redraw"
    );
}

// @guarantee LivenessRefreshedPeriodically: the tick loop emits a liveness check
// on a fixed multiple of the tick (config.main_session_poll_interval = 5 ticks),
// not on every tick.
#[test]
fn tick_polls_main_session_liveness_every_fifth_tick() {
    let mut app = make_app();
    let is_check = |cmds: &[Command]| {
        cmds.iter().any(|c| {
            matches!(
                c,
                Command::MainSession(crate::tui::commands::MainSessionCommand::CheckLiveness)
            )
        })
    };
    // Ticks 1–4 must not poll.
    for n in 1..=4 {
        let cmds = app.handle_tick();
        assert!(!is_check(&cmds), "tick {n} should not poll liveness");
    }
    // The 5th tick polls.
    let cmds = app.handle_tick();
    assert!(
        is_check(&cmds),
        "5th tick should poll main-session liveness"
    );
}
