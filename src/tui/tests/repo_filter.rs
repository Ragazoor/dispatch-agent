use super::*;
use crate::models::{test_tmux_window, Epic, EpicId, SubStatus, TaskId, TaskStatus};
use crate::tui::commands::SettingsCommand;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn start_repo_filter_enters_mode() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Start,
    ));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn toggle_repo_filter_adds_and_removes() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Toggle("/repo-a".to_string()),
    ));
    assert!(app.filter.repos.contains("/repo-a"));
    assert!(!app.filter.repos.contains("/repo-b"));

    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Toggle("/repo-a".to_string()),
    ));
    assert!(!app.filter.repos.contains("/repo-a"));
}

#[test]
fn toggle_all_repo_filter_selects_all_then_clears() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;

    // Toggle all on
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::ToggleAll,
    ));
    assert_eq!(app.filter.repos.len(), 2);

    // Toggle all off
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::ToggleAll,
    ));
    assert!(app.filter.repos.is_empty());
}

#[test]
fn close_repo_filter_returns_to_normal() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    let cmds = app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Close,
    ));
    assert_eq!(app.input.mode, InputMode::Normal);
    // Should emit PersistStringSetting
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Settings(SettingsCommand::PersistStringSetting { .. })
    )));
}

#[test]
fn flattened_board_respects_repo_filter() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    let mut in_repo = make_task(1, TaskStatus::Running);
    in_repo.epic_id = Some(EpicId(10));
    in_repo.repo_path = "/included".to_string();
    let mut out_repo = make_task(2, TaskStatus::Running);
    out_repo.epic_id = Some(EpicId(10));
    out_repo.repo_path = "/excluded".to_string();
    app.board.tasks = vec![in_repo, out_repo];
    app.board.flattened = true;
    app.filter.repos = vec!["/included".to_string()].into_iter().collect();

    let visible = app.tasks_for_current_view();
    let ids: std::collections::HashSet<_> = visible.iter().map(|t| t.id).collect();
    assert!(ids.contains(&TaskId(1)));
    assert!(!ids.contains(&TaskId(2)));
}

#[test]
fn repo_filter_empty_shows_all_tasks() {
    let app = make_app();
    // repo_filter is empty by default => all tasks visible
    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 4); // tasks 1,2,3,4 (Done tasks are visible, only Archived are excluded)
}

#[test]
fn repo_filter_hides_non_matching_tasks() {
    let mut app = App::new(vec![]);
    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.repo_path = "/repo-a".to_string();
    let mut t2 = make_task(2, TaskStatus::Backlog);
    t2.repo_path = "/repo-b".to_string();
    app.board.tasks = vec![t1, t2];
    app.filter.repos.insert("/repo-a".to_string());

    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, TaskId(1));
}

#[test]
fn repo_filter_applies_to_epics_in_column_items() {
    let mut app = App::new(vec![]);
    let now = chrono::Utc::now();
    app.board.epics = vec![
        Epic {
            id: EpicId(1),
            title: "A".into(),
            description: "".into(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: true,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: now,
            updated_at: now,
        },
        Epic {
            id: EpicId(2),
            title: "B".into(),
            description: "".into(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: true,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: now,
            updated_at: now,
        },
    ];
    // Epic A has a task in /repo-a; epic B has a task in /repo-b.
    // Filtering by /repo-a should show only epic A.
    let mut task_a = make_task(1, TaskStatus::Backlog);
    task_a.epic_id = Some(EpicId(1));
    task_a.repo_path = "/repo-a".to_string();
    let mut task_b = make_task(2, TaskStatus::Backlog);
    task_b.epic_id = Some(EpicId(2));
    task_b.repo_path = "/repo-b".to_string();
    app.board.tasks = vec![task_a, task_b];
    app.filter.repos.insert("/repo-a".to_string());

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert_eq!(items.len(), 1); // only epic A
}

#[test]
fn repo_filter_applies_to_epics_in_visual_column_items() {
    // Same fixture as repo_filter_applies_to_epics_in_column_items, but exercised
    // through column_items_for_visual_column (the split-pane layout builder) rather
    // than column_items_for_status (the flat-board builder), since the two used to
    // apply the repo filter to epics inconsistently.
    let mut app = App::new(vec![]);
    let now = chrono::Utc::now();
    app.board.epics = vec![
        Epic {
            id: EpicId(1),
            title: "A".into(),
            description: "".into(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: true,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: now,
            updated_at: now,
        },
        Epic {
            id: EpicId(2),
            title: "B".into(),
            description: "".into(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: true,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: now,
            updated_at: now,
        },
    ];
    let mut task_a = make_task(1, TaskStatus::Backlog);
    task_a.epic_id = Some(EpicId(1));
    task_a.repo_path = "/repo-a".to_string();
    let mut task_b = make_task(2, TaskStatus::Backlog);
    task_b.epic_id = Some(EpicId(2));
    task_b.repo_path = "/repo-b".to_string();
    app.board.tasks = vec![task_a, task_b];
    app.filter.repos.insert("/repo-a".to_string());

    // vcol_idx 0 is the Backlog visual column (VisualColumn::parent_group_start).
    let items = app.column_items_for_visual_column(0);
    assert_eq!(items.len(), 1); // only epic A
}

#[test]
fn visual_column_sorts_by_sub_status_urgency_before_id() {
    // Regression guard: column_items_for_visual_column previously sorted only
    // by (sort_order.unwrap_or(MAX), id), ignoring sub-status urgency. In a
    // mixed visual column (vcol 3 = Stale/Crashed/Conflict) a lower-id Stale
    // task must not outrank a higher-id Conflict task — Conflict is more
    // urgent and must sort first, matching the flat-board builder.
    let mut app = App::new(vec![]);
    let mut stale_task = make_task(1, TaskStatus::Running);
    stale_task.sub_status = SubStatus::Stale;
    let mut conflict_task = make_task(2, TaskStatus::Running);
    conflict_task.sub_status = SubStatus::Conflict;
    app.board.tasks = vec![stale_task, conflict_task];

    // vcol_idx 3 is the "Stale" visual column (Stale/Crashed/Conflict).
    let items = app.column_items_for_visual_column(3);
    let ids: Vec<i64> = items
        .iter()
        .filter_map(|item| match item {
            ColumnItem::Task(t) => Some(t.id.0),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        vec![2, 1],
        "Conflict (id 2) must sort before Stale (id 1) by urgency, not id"
    );
}

#[test]
fn f_key_opens_repo_filter() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.handle_key(make_key(KeyCode::Char('f')));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn repo_filter_number_key_toggles_repo() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('1')));
    assert!(app.filter.repos.contains("/repo-a"));

    app.handle_key(make_key(KeyCode::Char('1')));
    assert!(!app.filter.repos.contains("/repo-a"));
}

#[test]
fn repo_filter_a_key_toggles_all() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('a')));
    assert_eq!(app.filter.repos.len(), 2);
}

#[test]
fn repo_filter_enter_closes() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Settings(SettingsCommand::PersistStringSetting { .. })
    )));
}

#[test]
fn repo_filter_esc_closes() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn repo_filter_out_of_range_number_ignored() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('5')));
    assert!(app.filter.repos.is_empty());
}

#[test]
fn repo_filter_exclude_hides_matching_tasks() {
    let mut app = App::new(vec![]);
    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.repo_path = "/repo-a".to_string();
    let mut t2 = make_task(2, TaskStatus::Backlog);
    t2.repo_path = "/repo-b".to_string();
    app.board.tasks = vec![t1, t2];
    app.filter.repos.insert("/repo-a".to_string());
    app.filter.mode = RepoFilterMode::Exclude;

    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, TaskId(2));
}

#[test]
fn repo_filter_exclude_empty_shows_all() {
    let mut app = App::new(vec![]);
    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.repo_path = "/repo-a".to_string();
    let mut t2 = make_task(2, TaskStatus::Backlog);
    t2.repo_path = "/repo-b".to_string();
    app.board.tasks = vec![t1, t2];
    app.filter.mode = RepoFilterMode::Exclude;

    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 2);
}

#[test]
fn repo_filter_exclude_applies_to_epics() {
    let mut app = App::new(vec![]);
    let now = chrono::Utc::now();
    app.board.epics = vec![
        Epic {
            id: EpicId(1),
            title: "A".into(),
            description: "".into(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: true,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: now,
            updated_at: now,
        },
        Epic {
            id: EpicId(2),
            title: "B".into(),
            description: "".into(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: true,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: now,
            updated_at: now,
        },
    ];
    // Epic A has a task in /repo-a; epic B has a task in /repo-b.
    // Excluding /repo-a should hide epic A and show only epic B.
    let mut task_a = make_task(1, TaskStatus::Backlog);
    task_a.epic_id = Some(EpicId(1));
    task_a.repo_path = "/repo-a".to_string();
    let mut task_b = make_task(2, TaskStatus::Backlog);
    task_b.epic_id = Some(EpicId(2));
    task_b.repo_path = "/repo-b".to_string();
    app.board.tasks = vec![task_a, task_b];
    app.filter.repos.insert("/repo-a".to_string());
    app.filter.mode = RepoFilterMode::Exclude;

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert_eq!(items.len(), 1);
    match &items[0] {
        ColumnItem::Epic(e) => assert_eq!(e.id, EpicId(2)),
        _ => panic!("Expected epic"),
    }
}

#[test]
fn close_repo_filter_persists_mode() {
    let mut app = make_app();
    app.filter.mode = RepoFilterMode::Exclude;
    app.input.mode = InputMode::RepoFilter;
    let cmds = app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Close,
    ));
    let expected_key = "repo_filter_mode";
    assert!(cmds.iter().any(|c| matches!(c,
        Command::Settings(SettingsCommand::PersistStringSetting { key, value }) if *key == expected_key && value == "exclude"
    )));
}

#[test]
fn close_repo_filter_persists_keys() {
    // Closing repo filter should persist repo_filter and repo_filter_mode settings.
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::RepoFilter;
    let cmds = app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Close,
    ));
    let want_filter = "repo_filter";
    let want_mode = "repo_filter_mode";
    let keys: Vec<&str> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::Settings(SettingsCommand::PersistStringSetting { key, .. }) => {
                Some(key.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(
        keys.contains(&want_filter),
        "Expected key {want_filter} in {keys:?}"
    );
    assert!(
        keys.contains(&want_mode),
        "Expected key {want_mode} in {keys:?}"
    );
}

#[test]
fn repo_filter_overlay_shows_mode_in_title() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.filter.mode = RepoFilterMode::Exclude;
    app.input.mode = InputMode::RepoFilter;

    let buf = render_to_buffer(&mut app, 80, 25);
    assert!(
        buffer_contains(&buf, "exclude"),
        "Expected 'exclude' in overlay title"
    );
}

#[test]
fn load_filter_preset_replaces_repo_filter() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.filter.repos.insert("/repo-a".to_string());

    let preset_repos: HashSet<String> = ["/repo-b".to_string()].into_iter().collect();
    app.filter.presets = vec![("backend".to_string(), preset_repos, RepoFilterMode::Include)];

    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::LoadPreset("backend".to_string()),
    ));
    assert!(app.filter.repos.contains("/repo-b"));
    assert!(!app.filter.repos.contains("/repo-a"));
}

#[test]
fn cancel_preset_input_returns_to_repo_filter() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.input.set_buffer("draft".to_string());
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::CancelPresetInput,
    ));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(app.input.buffer.is_empty());
}

#[test]
fn repo_filter_s_key_starts_save_preset() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Char('s')));
    assert_eq!(app.input.mode, InputMode::InputPresetName);
}

#[test]
fn repo_filter_x_key_starts_delete_preset() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    app.filter.presets = vec![("test".to_string(), repos, RepoFilterMode::Include)];
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmDeletePreset);
}

#[test]
fn repo_filter_shift_a_loads_first_preset() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    let repos: HashSet<String> = ["/repo-b".to_string()].into_iter().collect();
    app.filter.presets = vec![("backend".to_string(), repos, RepoFilterMode::Include)];
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
    assert!(app.filter.repos.contains("/repo-b"));
    assert!(!app.filter.repos.contains("/repo-a"));
}

#[test]
fn repo_filter_overlay_shows_presets() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    let repos: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.filter.presets = vec![("frontend".to_string(), repos, RepoFilterMode::Include)];
    app.input.mode = InputMode::RepoFilter;

    let buf = render_to_buffer(&mut app, 80, 25);
    assert!(buffer_contains(&buf, "A"), "Expected preset letter A");
    assert!(
        buffer_contains(&buf, "frontend"),
        "Expected preset name 'frontend'"
    );
}

#[test]
fn repo_filter_overlay_shows_name_input() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::InputPresetName;
    app.input.set_buffer("myfilter".to_string());

    let buf = render_to_buffer(&mut app, 80, 25);
    assert!(buffer_contains(&buf, "Name:"), "Expected name input prompt");
    assert!(buffer_contains(&buf, "myfilter"), "Expected buffer content");
}

#[test]
fn repo_filter_overlay_shows_delete_help() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo-a".to_string()];
    let repos: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.filter.presets = vec![("test".to_string(), repos, RepoFilterMode::Include)];
    app.input.mode = InputMode::ConfirmDeletePreset;

    let buf = render_to_buffer(&mut app, 80, 25);
    assert!(
        buffer_contains(&buf, "delete preset"),
        "Expected delete help text"
    );
}

#[test]
fn handle_key_repo_filter_toggle() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('1')));
    assert!(app.filter.repos.contains("/repo"));
}

#[test]
fn handle_key_repo_filter_close_enter() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_repo_filter_close_esc() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_repo_filter_close_q() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(make_key(KeyCode::Char('q')));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn repo_filter_j_moves_cursor_down() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 0;
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.input.repo_cursor, 1);
}

#[test]
fn repo_filter_space_toggles_cursor_repo() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 2; // cursor 2 = repo index 1 = /repo-b
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(
        app.filter.repos.contains("/repo-b"),
        "cursor repo should be toggled"
    );
    assert!(!app.filter.repos.contains("/repo-a"));
}

#[test]
fn render_status_bar_repo_filter() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Filter repos"),
        "RepoFilter mode should show 'Filter repos'"
    );
}

#[test]
fn render_repo_filter_overlay_shows_title() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo/a".to_string(), "/repo/b".to_string()];
    app.input.mode = InputMode::RepoFilter;
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        buffer_contains(&buf, "Repo Filter"),
        "repo filter overlay should show 'Repo Filter' title"
    );
}

#[test]
fn render_repo_filter_overlay_shows_repos() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo/alpha".to_string(), "/repo/beta".to_string()];
    app.input.mode = InputMode::RepoFilter;
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        buffer_contains(&buf, "/repo/alpha"),
        "repo filter overlay should show '/repo/alpha'"
    );
    assert!(
        buffer_contains(&buf, "/repo/beta"),
        "repo filter overlay should show '/repo/beta'"
    );
}

#[test]
fn render_repo_filter_overlay_shows_include_mode() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo/a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        buffer_contains(&buf, "include"),
        "repo filter overlay should show 'include' as default mode"
    );
}

#[test]
fn render_repo_filter_overlay_shows_navigate_hint() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::RepoFilter;
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        buffer_contains(&buf, "navigate"),
        "repo filter overlay should show 'navigate' hint"
    );
}

#[test]
fn render_repo_filter_input_preset_name() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo/a".to_string()];
    app.input.mode = InputMode::InputPresetName;
    app.input.set_buffer("my-preset".to_string());
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        buffer_contains(&buf, "Name:"),
        "preset name input should show 'Name:' label"
    );
    assert!(
        buffer_contains(&buf, "my-preset"),
        "preset name input should show the buffer content 'my-preset'"
    );
}

#[test]
fn render_repo_filter_confirm_delete_preset() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo/a".to_string()];
    app.input.mode = InputMode::ConfirmDeletePreset;
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        buffer_contains(&buf, "delete preset"),
        "confirm delete mode should show 'delete preset' text"
    );
}

#[test]
fn start_delete_repo_path_enters_confirm_mode() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::StartDeleteRepoPath,
    ));
    assert_eq!(app.input.mode, InputMode::ConfirmDeleteRepoPath);
}

#[test]
fn start_delete_repo_path_no_repos_is_noop() {
    let mut app = make_app();
    app.board.repo_paths = vec![];
    app.input.mode = InputMode::RepoFilter;
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::StartDeleteRepoPath,
    ));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn confirm_delete_repo_path_emits_command() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::ConfirmDeleteRepoPath;
    app.input.repo_cursor = 1;
    let cmds = app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::DeleteRepoPath("/repo-b".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::RepoFilter(crate::tui::commands::RepoFilterCommand::DeleteRepoPath(p)) if p == "/repo-b")));
}

#[test]
fn cancel_delete_repo_path_returns_to_filter() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::ConfirmDeleteRepoPath;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn delete_repo_path_removes_from_active_filter() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.filter.repos.insert("/repo-a".to_string());
    app.filter.repos.insert("/repo-b".to_string());
    app.input.mode = InputMode::ConfirmDeleteRepoPath;
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::DeleteRepoPath("/repo-a".to_string()),
    ));
    assert!(!app.filter.repos.contains("/repo-a"));
    assert!(app.filter.repos.contains("/repo-b"));
}

#[test]
fn delete_repo_path_clamps_cursor() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    // cursor 2 = second repo in new model (cursor 1..=N where N=repo count)
    app.input.repo_cursor = 2;
    // Simulate the path being removed (RepoPathsUpdated would do this in practice)
    app.update(Message::RepoPathsUpdated(vec!["/repo-a".to_string()]));
    // cursor 0..=1 valid for 1 repo; cursor 2 should clamp to 1
    assert_eq!(
        app.input.repo_cursor, 1,
        "cursor should be clamped to len when repo list shrinks"
    );
}

#[test]
fn backspace_in_repo_filter_starts_delete() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0, so Backspace will trigger
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.mode, InputMode::ConfirmDeleteRepoPath);
}

#[test]
fn delete_key_in_repo_filter_starts_delete() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0, so Delete will trigger
    app.handle_key(make_key(KeyCode::Delete));
    assert_eq!(app.input.mode, InputMode::ConfirmDeleteRepoPath);
}

#[test]
fn y_in_confirm_delete_repo_path_confirms() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::ConfirmDeleteRepoPath;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0 = /repo-a
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::RepoFilter(crate::tui::commands::RepoFilterCommand::DeleteRepoPath(p)) if p == "/repo-a")));
}

#[test]
fn n_in_confirm_delete_repo_path_cancels() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::ConfirmDeleteRepoPath;
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn handle_key_normal_start_repo_filter() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('f')));
    assert_eq!(*app.mode(), InputMode::RepoFilter);
}

#[test]
fn handle_key_repo_filter_a_toggles_all() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('a')));
    // Should toggle all repos in filter
    assert!(!app.filter.repos.is_empty());
}

#[test]
fn handle_key_repo_filter_space_toggles_cursor_item() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0 = /repo

    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.filter.repos.contains("/repo"));
}

#[test]
fn handle_key_repo_filter_j_moves_cursor() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 0;

    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.input.repo_cursor, 1);
}

#[test]
fn handle_key_repo_filter_k_moves_cursor() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 1;

    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn handle_key_repo_filter_backspace_starts_delete_repo_path() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0, so Backspace will trigger

    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(*app.mode(), InputMode::ConfirmDeleteRepoPath);
}

#[test]
fn handle_key_repo_filter_s_starts_save_preset() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('s')));
    assert_eq!(*app.mode(), InputMode::InputPresetName);
}

#[test]
fn handle_key_repo_filter_x_starts_delete_preset() {
    let mut app = make_app();
    app.filter.presets = vec![(
        "preset-a".to_string(),
        std::collections::HashSet::new(),
        RepoFilterMode::Include,
    )];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(*app.mode(), InputMode::ConfirmDeletePreset);
}

#[test]
fn handle_key_repo_filter_uppercase_loads_preset() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.filter.presets = vec![(
        "preset-a".to_string(),
        {
            let mut s = std::collections::HashSet::new();
            s.insert("/repo".to_string());
            s
        },
        RepoFilterMode::Include,
    )];
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('A')));
    assert!(app.filter.repos.contains("/repo"));
}

#[test]
fn handle_key_repo_filter_uppercase_out_of_range_is_noop() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    // No presets
    let cmds = app.handle_key(make_key(KeyCode::Char('A')));
    assert!(cmds.is_empty());
}

#[test]
fn handle_key_repo_filter_unknown_key_is_noop() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    let cmds = app.handle_key(make_key(KeyCode::Char('z')));
    assert!(cmds.is_empty());
}

#[test]
fn toggle_repo_filter_mode_switches() {
    let mut app = make_app();
    assert_eq!(app.filter.mode, RepoFilterMode::Include);
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::ToggleMode,
    ));
    assert_eq!(app.filter.mode, RepoFilterMode::Exclude);
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::ToggleMode,
    ));
    assert_eq!(app.filter.mode, RepoFilterMode::Include);
}

#[test]
fn tab_key_toggles_repo_filter_mode() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::RepoFilter;
    assert_eq!(app.filter.mode, RepoFilterMode::Include);
    app.handle_key(make_key(KeyCode::Tab));
    assert_eq!(app.filter.mode, RepoFilterMode::Exclude);
    app.handle_key(make_key(KeyCode::Tab));
    assert_eq!(app.filter.mode, RepoFilterMode::Include);
}

#[test]
fn repo_filter_overlay_shows_tab_hint() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo/a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        buffer_contains(&buf, "Tab"),
        "repo filter overlay should show [Tab] hint for toggling include/exclude"
    );
}

#[test]
fn handle_key_confirm_delete_repo_path_y_deletes() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::ConfirmDeleteRepoPath;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0 = /repo

    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::RepoFilter(crate::tui::commands::RepoFilterCommand::DeleteRepoPath(_))
    )));
}

#[test]
fn handle_key_confirm_delete_repo_path_other_cancels() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::ConfirmDeleteRepoPath;

    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(*app.mode(), InputMode::RepoFilter);
}

#[test]
fn handle_key_confirm_delete_repo_path_uppercase_y() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::ConfirmDeleteRepoPath;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0 = /repo

    let cmds = app.handle_key(make_key(KeyCode::Char('Y')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::RepoFilter(crate::tui::commands::RepoFilterCommand::DeleteRepoPath(_))
    )));
}

/// RepoFilter mode routes correctly.
#[test]
fn handle_key_repo_filter_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    // Esc closes the filter (may emit refresh commands)
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
}

// ---------------------------------------------------------------------------
// only_active field and task_matches tests
// ---------------------------------------------------------------------------

#[test]
fn filter_state_task_matches_only_active_false_always_passes() {
    use crate::tui::FilterState;
    let state = FilterState::default();
    assert!(!state.only_active);
    let mut t = make_task(1, TaskStatus::Running);
    t.tmux_window = None;
    assert!(state.task_matches(&t));
    t.tmux_window = Some(test_tmux_window("dispatch:1"));
    assert!(state.task_matches(&t));
}

#[test]
fn filter_state_task_matches_only_active_true_requires_tmux_window() {
    use crate::tui::FilterState;
    let state = FilterState {
        only_active: true,
        ..Default::default()
    };

    let mut t = make_task(1, TaskStatus::Running);
    t.tmux_window = None;
    assert!(!state.task_matches(&t));

    t.tmux_window = Some(test_tmux_window("dispatch:1"));
    assert!(state.task_matches(&t));
}

/// ConfirmDeleteRepoPath mode routes correctly.
#[test]
fn handle_key_confirm_delete_repo_path_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDeleteRepoPath;
    // Any non-y key returns to RepoFilter
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn toggle_only_active_flips_flag() {
    let mut app = make_app();
    assert!(!app.filter.only_active);

    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::ToggleOnlyActive,
    ));
    assert!(app.filter.only_active);

    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::ToggleOnlyActive,
    ));
    assert!(!app.filter.only_active);
}

#[test]
fn repo_filter_cursor_zero_is_toggle_row() {
    // After opening the filter, cursor starts at 0 (toggle row).
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Start,
    ));
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn repo_filter_cursor_navigates_past_toggle_row() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 0;

    // Down from 0 → 1 (first repo)
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::MoveCursor(1),
    ));
    assert_eq!(app.input.repo_cursor, 1);

    // Down again → 2 (second repo)
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::MoveCursor(1),
    ));
    assert_eq!(app.input.repo_cursor, 2);

    // Down from last repo wraps to 0 (toggle row)
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::MoveCursor(1),
    ));
    assert_eq!(app.input.repo_cursor, 0);

    // Up from 0 wraps to last repo
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::MoveCursor(-1),
    ));
    assert_eq!(app.input.repo_cursor, 2);
}

#[test]
fn repo_filter_cursor_navigates_with_no_repos() {
    // With zero repos, only the toggle row exists at cursor 0.
    let mut app = make_app();
    app.board.repo_paths = vec![];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 0;

    // Moving up or down stays at 0 (wraps within single item)
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::MoveCursor(1),
    ));
    assert_eq!(app.input.repo_cursor, 0);
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::MoveCursor(-1),
    ));
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
#[ignore = "requires a real TTY and interactive editor session; run manually to verify"]
fn buffered_editor_keystrokes_do_not_leak_into_repo_picker() {}

#[test]
fn repo_filter_overlay_shows_toggle_row() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;

    let buf = render_to_buffer(&mut app, 80, 30);
    assert!(
        buffer_contains(&buf, "Active sessions only"),
        "overlay should show the toggle row"
    );
}

#[test]
fn repo_filter_overlay_toggle_row_shows_checked_when_active() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.filter.only_active = true;

    let buf = render_to_buffer(&mut app, 80, 30);
    assert!(
        buffer_contains(&buf, "[x] Active sessions only"),
        "toggle row should show [x] when only_active is true"
    );
}

#[test]
fn repo_filter_overlay_toggle_row_has_cursor_indicator_at_cursor_zero() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 0;

    let buf = render_to_buffer(&mut app, 80, 30);
    assert!(
        buffer_contains(&buf, "► [ ] Active sessions only"),
        "cursor indicator should appear on toggle row when cursor == 0"
    );
}

#[test]
fn repo_filter_overlay_repo_row_has_cursor_at_cursor_one() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0

    let buf = render_to_buffer(&mut app, 80, 30);
    assert!(
        buffer_contains(&buf, "►"),
        "cursor indicator should appear on repo row when cursor == 1"
    );
}

// ---------------------------------------------------------------------------
// Direct keybinding for only_active filter
// ---------------------------------------------------------------------------

#[test]
fn shift_a_in_normal_mode_toggles_only_active() {
    let mut app = make_app();
    assert!(!app.filter.only_active);

    app.handle_key(make_key(KeyCode::Char('A')));
    assert!(app.filter.only_active);

    app.handle_key(make_key(KeyCode::Char('A')));
    assert!(!app.filter.only_active);
}

#[test]
fn shift_a_does_not_toggle_only_active_in_repo_filter_mode() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;

    app.handle_key(make_key(KeyCode::Char('A')));
    assert!(
        !app.filter.only_active,
        "A in repo filter mode should not toggle only_active (it loads preset index 0)"
    );
}

#[test]
fn shift_a_in_epic_view_toggles_only_active() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    app.board.view_mode = crate::tui::types::ViewMode::Epic {
        epic_id: EpicId(10),
        selection: crate::tui::types::BoardSelection::new_for_epic(),
        parent: Box::new(crate::tui::types::ViewMode::Board(
            crate::tui::types::BoardSelection::new(),
        )),
    };

    assert!(!app.filter.only_active);
    app.handle_key(make_key(KeyCode::Char('A')));
    assert!(app.filter.only_active);
}

// ---------------------------------------------------------------------------
// Epic-in-epic: TUI navigation tests
// ---------------------------------------------------------------------------

#[test]
fn space_at_cursor_zero_toggles_only_active() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 0;

    assert!(!app.filter.only_active);
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.filter.only_active);

    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(!app.filter.only_active);
}

#[test]
fn space_at_cursor_one_toggles_first_repo() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0

    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.filter.repos.contains("/repo-a"));
    assert!(!app.filter.repos.contains("/repo-b"));
}

#[test]
fn backspace_at_cursor_zero_is_noop() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 0;

    // Should not enter ConfirmDeleteRepoPath mode
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn backspace_at_cursor_one_starts_delete() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.input.mode = InputMode::RepoFilter;
    app.input.repo_cursor = 1; // cursor 1 = repo index 0

    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.mode, InputMode::ConfirmDeleteRepoPath);
}

#[test]
fn enter_sub_epic_from_epic_view_nests_parent() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(1), make_epic(2)];
    // Start in Epic view for epic 1
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(1),
    )));
    app.selection_mut().set_column(2);

    // Enter sub-epic 2 from within epic 1
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(2),
    )));

    match &app.board.view_mode {
        ViewMode::Epic {
            epic_id, parent, ..
        } => {
            assert_eq!(*epic_id, EpicId(2), "should be in sub-epic 2");
            match parent.as_ref() {
                ViewMode::Epic {
                    epic_id: parent_id, ..
                } => {
                    assert_eq!(*parent_id, EpicId(1), "parent should be epic 1");
                }
                _ => panic!("Expected parent to be ViewMode::Epic"),
            }
        }
        _ => panic!("Expected ViewMode::Epic"),
    }
}

// ---------------------------------------------------------------------------
// Task filtering with only_active
// ---------------------------------------------------------------------------

#[test]
fn only_active_filter_hides_tasks_without_tmux_window() {
    let mut app = App::new(vec![]);
    let mut t1 = make_task(1, TaskStatus::Running);
    t1.tmux_window = Some(test_tmux_window("dispatch:1"));
    let mut t2 = make_task(2, TaskStatus::Running);
    t2.tmux_window = None;
    let mut t3 = make_task(3, TaskStatus::Backlog);
    t3.tmux_window = None;
    app.board.tasks = vec![t1, t2, t3];
    app.filter.only_active = true;

    let visible = app.tasks_for_current_view();
    let ids: Vec<_> = visible.iter().map(|t| t.id.0).collect();
    assert_eq!(ids, vec![1], "only task with tmux_window should be visible");
}

#[test]
fn only_active_filter_off_shows_all_tasks() {
    let mut app = App::new(vec![]);
    let mut t1 = make_task(1, TaskStatus::Running);
    t1.tmux_window = Some(test_tmux_window("dispatch:1"));
    let mut t2 = make_task(2, TaskStatus::Backlog);
    t2.tmux_window = None;
    app.board.tasks = vec![t1, t2];

    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 2);
}

#[test]
fn status_bar_shows_active_badge_when_only_active_enabled() {
    let mut app = make_app();
    app.filter.only_active = true;
    app.input.mode = InputMode::Normal;

    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "[active]"),
        "status bar should show [active] badge when only_active is set"
    );
}

#[test]
fn status_bar_no_active_badge_when_only_active_disabled() {
    let mut app = make_app();
    app.filter.only_active = false;
    app.input.mode = InputMode::Normal;

    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        !buffer_contains(&buf, "[active]"),
        "status bar should not show [active] badge when only_active is false"
    );
}

// Epic filtering with only_active
// ---------------------------------------------------------------------------

#[test]
fn only_active_filter_hides_epic_with_no_active_tasks() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Backlog;
    let mut t = make_task(1, TaskStatus::Backlog);
    t.epic_id = Some(EpicId(10));
    t.tmux_window = None;
    app.board.epics = vec![epic];
    app.board.tasks = vec![t];
    app.filter.only_active = true;

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(
        items.iter().all(|i| !matches!(i, ColumnItem::Epic(_))),
        "epic with no active tasks should be hidden when only_active is set"
    );
}

#[test]
fn only_active_filter_shows_epic_with_active_task() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Backlog;
    let mut t = make_task(1, TaskStatus::Backlog);
    t.epic_id = Some(EpicId(10));
    t.tmux_window = Some(test_tmux_window("dispatch:1"));
    app.board.epics = vec![epic];
    app.board.tasks = vec![t];
    app.filter.only_active = true;

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(
        items
            .iter()
            .any(|i| matches!(i, ColumnItem::Epic(e) if e.id == EpicId(10))),
        "epic with an active task should be visible when only_active is set"
    );
}

#[test]
fn only_active_filter_off_shows_epics_without_active_tasks() {
    let mut app = App::new(vec![]);
    let mut epic = make_epic(10);
    epic.status = TaskStatus::Backlog;
    app.board.epics = vec![epic];
    app.filter.only_active = false;

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(
        items
            .iter()
            .any(|i| matches!(i, ColumnItem::Epic(e) if e.id == EpicId(10))),
        "epic should be visible when only_active is off"
    );
}

#[test]
fn only_active_filter_column_item_count_excludes_inactive_epics() {
    let mut app = App::new(vec![]);
    let mut epic_active = make_epic(10);
    epic_active.status = TaskStatus::Backlog;
    let mut epic_inactive = make_epic(20);
    epic_inactive.status = TaskStatus::Backlog;

    let mut t = make_task(1, TaskStatus::Backlog);
    t.epic_id = Some(EpicId(10));
    t.tmux_window = Some(test_tmux_window("dispatch:1"));

    app.board.epics = vec![epic_active, epic_inactive];
    app.board.tasks = vec![t];
    app.filter.only_active = true;

    let count = app.column_item_count(TaskStatus::Backlog);
    assert_eq!(
        count, 1,
        "only the epic with an active task should be counted"
    );
}

#[test]
fn only_active_filter_shows_root_epic_when_grandchild_task_is_active() {
    // epic(10) → sub-epic(20) → task(with tmux_window)
    // Root epic should be visible because a descendant task is active.
    let mut app = App::new(vec![]);

    let mut root_epic = make_epic(10);
    root_epic.status = TaskStatus::Backlog;

    let mut sub_epic = make_epic(20);
    sub_epic.status = TaskStatus::Backlog;
    sub_epic.parent_epic_id = Some(EpicId(10));

    let mut t = make_task(1, TaskStatus::Backlog);
    t.epic_id = Some(EpicId(20));
    t.tmux_window = Some(test_tmux_window("dispatch:1"));

    app.board.epics = vec![root_epic, sub_epic];
    app.board.tasks = vec![t];
    app.filter.only_active = true;

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(
        items
            .iter()
            .any(|i| matches!(i, ColumnItem::Epic(e) if e.id == EpicId(10))),
        "root epic should be visible when a grandchild task is active"
    );
}

#[test]
fn filter_and_view_changes_reset_column_scroll_offsets() {
    fn check(label: &str, update: impl FnOnce(&mut App)) {
        let mut app = make_app();
        app.board.repo_paths = vec!["/repo".to_string()];
        app.filter.presets = vec![(
            "my-preset".to_string(),
            std::collections::HashSet::new(),
            crate::tui::types::RepoFilterMode::Include,
        )];
        for state in &mut app.selection_mut().list_states {
            *state.offset_mut() = 5;
        }
        update(&mut app);
        for state in &app.selection().list_states {
            assert_eq!(
                state.offset(),
                0,
                "{label} should reset column scroll offsets"
            );
        }
    }

    check("ToggleRepoFilter", |app| {
        app.update(Message::RepoFilter(
            crate::tui::messages::RepoFilterMessage::Toggle("/repo".to_string()),
        ));
    });
    check("ToggleOnlyActive", |app| {
        app.update(Message::RepoFilter(
            crate::tui::messages::RepoFilterMessage::ToggleOnlyActive,
        ));
    });
    check("CloseRepoFilter", |app| {
        app.input.mode = InputMode::RepoFilter;
        app.update(Message::RepoFilter(
            crate::tui::messages::RepoFilterMessage::Close,
        ));
    });
    check("ToggleRepoFilterMode", |app| {
        app.update(Message::RepoFilter(
            crate::tui::messages::RepoFilterMessage::ToggleMode,
        ));
    });
    check("ToggleAllRepoFilter", |app| {
        app.update(Message::RepoFilter(
            crate::tui::messages::RepoFilterMessage::ToggleAll,
        ));
    });
    check("LoadFilterPreset", |app| {
        app.update(Message::RepoFilter(
            crate::tui::messages::RepoFilterMessage::LoadPreset("my-preset".to_string()),
        ));
    });
    check("ToggleFlattened", |app| {
        app.update(Message::Task(
            crate::tui::messages::TaskMessage::ToggleFlattened,
        ));
    });
}

// ---------------------------------------------------------------------------
// broken_repo_paths cache
// ---------------------------------------------------------------------------

#[test]
fn broken_repo_paths_marks_nonexistent_path_on_update() {
    let mut app = make_app();
    app.update(Message::RepoPathsUpdated(vec![
        "/tmp".to_string(),
        "/this-path-absolutely-does-not-exist-dispatch-test".to_string(),
    ]));
    assert!(
        app.broken_repo_paths
            .contains("/this-path-absolutely-does-not-exist-dispatch-test"),
        "non-existent path should be in broken_repo_paths"
    );
    assert!(
        !app.broken_repo_paths.contains("/tmp"),
        "/tmp is a real directory and must not appear in broken_repo_paths"
    );
}

#[test]
fn broken_repo_paths_replaced_on_second_update() {
    let mut app = make_app();
    app.update(Message::RepoPathsUpdated(vec![
        "/this-path-absolutely-does-not-exist-dispatch-test".to_string(),
    ]));
    assert!(!app.broken_repo_paths.is_empty());

    // Replace with only real paths — cache must be cleared
    app.update(Message::RepoPathsUpdated(vec!["/tmp".to_string()]));
    assert!(
        app.broken_repo_paths.is_empty(),
        "broken_repo_paths must be empty after update with only valid paths"
    );
}
