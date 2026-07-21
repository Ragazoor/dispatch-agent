#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{SubStatus, TaskId, TaskStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, Instant};

#[test]
fn resumed_sets_success_status_message() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/wt".to_string());
    let mut app = App::new(vec![task]);

    app.update(Message::Task(crate::tui::messages::TaskMessage::Resumed {
        id: TaskId(4),
        tmux_window: "win-4".to_string(),
    }));

    assert_eq!(app.status.message.as_deref(), Some("Task 4 resumed"),);
}

#[test]
fn batch_move_multiple_steps() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    // Move Backlog -> Running (clears selection)
    app.handle_key(make_key(KeyCode::Char('L')));

    // Re-select and move Running -> Review
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));
    app.handle_key(make_key(KeyCode::Char('L')));

    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Review);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Review);
}

#[test]
fn render_status_bar_shows_keybindings() {
    let mut app = App::new(vec![]);
    let buf = render_to_buffer(&mut app, 200, 20);
    assert!(buffer_contains(&buf, "[n]"));
}

#[test]
fn render_status_bar_uses_bracket_format() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let buf = render_to_buffer(&mut app, 220, 20);
    // Hints should use [key] bracket format
    assert!(
        buffer_contains(&buf, "[n]"),
        "status bar should use [key] bracket format"
    );
    assert!(
        buffer_contains(&buf, "[n]ew"),
        "status bar should show 'new' hint"
    );
}

#[test]
fn status_message_clears_after_timeout_on_tick() {
    let mut app = make_app();
    // Simulate a status message that was set 6 seconds ago
    app.status.message = Some("Task 1 finished".to_string());
    app.status.message_set_at = Some(Instant::now() - Duration::from_secs(6));

    // Tick should clear it since it's past the 5-second timeout
    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert!(
        app.status.message.is_none(),
        "status_message should auto-clear after timeout"
    );
}

#[test]
fn status_message_persists_before_timeout() {
    let mut app = make_app();
    // Set a message just now
    app.status.message = Some("Task 1 finished".to_string());
    app.status.message_set_at = Some(Instant::now());

    // Tick should NOT clear it since timeout hasn't elapsed
    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert_eq!(app.status.message.as_deref(), Some("Task 1 finished"));
}

#[test]
fn status_message_does_not_clear_during_interactive_mode() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDelete;
    app.status.message = Some("Delete task? [y/n]".to_string());
    app.status.message_set_at = Some(Instant::now() - Duration::from_secs(10));

    // Tick should NOT clear it during an interactive mode
    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert!(
        app.status.message.is_some(),
        "should not clear during interactive mode"
    );
}

#[test]
fn notifications_disabled_by_default() {
    let app = make_app();
    assert!(!app.notifications_enabled());
}

#[test]
fn save_filter_preset_stores_mode() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.filter.repos.insert("/repo-a".to_string());
    app.filter.mode = RepoFilterMode::Exclude;
    app.input.mode = InputMode::InputPresetName;

    let cmds = app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::SavePreset("excl-preset".to_string()),
    ));
    assert_eq!(app.filter.presets[0].2, RepoFilterMode::Exclude);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::RepoFilter(
            crate::tui::commands::RepoFilterCommand::PersistFilterPreset {
                mode: RepoFilterMode::Exclude,
                ..
            }
        )
    )));
}

#[test]
fn load_filter_preset_restores_mode() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    let repos: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.filter.presets = vec![("excl".to_string(), repos, RepoFilterMode::Exclude)];

    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::LoadPreset("excl".to_string()),
    ));
    assert_eq!(app.filter.mode, RepoFilterMode::Exclude);
    assert!(app.filter.repos.contains("/repo-a"));
}

#[test]
fn save_filter_preset_stores_and_persists() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.filter.repos.insert("/repo-a".to_string());
    app.input.mode = InputMode::RepoFilter;

    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::StartSavePreset,
    ));
    assert_eq!(app.input.mode, InputMode::InputPresetName);

    let cmds = app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::SavePreset("frontend".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert_eq!(app.filter.presets.len(), 1);
    assert_eq!(app.filter.presets[0].0, "frontend");
    assert!(app.filter.presets[0].1.contains("/repo-a"));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::RepoFilter(crate::tui::commands::RepoFilterCommand::PersistFilterPreset { .. })
    )));
}

#[test]
fn save_filter_preset_empty_name_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::SavePreset("  ".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(app.filter.presets.is_empty());
}

#[test]
fn save_filter_preset_overwrites_existing() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    let old: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.filter.presets = vec![("frontend".to_string(), old, RepoFilterMode::Include)];

    app.filter.repos.insert("/repo-b".to_string());
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::SavePreset("frontend".to_string()),
    ));
    assert_eq!(app.filter.presets.len(), 1);
    assert!(app.filter.presets[0].1.contains("/repo-b"));
}

#[test]
fn delete_filter_preset_removes_and_returns_command() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.filter.presets = vec![("frontend".to_string(), repos, RepoFilterMode::Include)];
    app.input.mode = InputMode::ConfirmDeletePreset;

    let cmds = app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::DeletePreset("frontend".to_string()),
    ));
    assert!(app.filter.presets.is_empty());
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::RepoFilter(crate::tui::commands::RepoFilterCommand::DeleteFilterPreset(
            _
        ))
    )));
}

#[test]
fn filter_presets_loaded_sets_state() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo-a".to_string()].into_iter().collect();
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::PresetsLoaded(vec![(
            "frontend".to_string(),
            repos.clone(),
            RepoFilterMode::Include,
        )]),
    ));
    assert_eq!(app.filter.presets.len(), 1);
    assert_eq!(app.filter.presets[0].0, "frontend");
}

#[test]
fn load_filter_preset_unknown_name_is_noop() {
    let mut app = make_app();
    app.filter.repos.insert("/repo-a".to_string());
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::LoadPreset("nonexistent".to_string()),
    ));
    assert!(app.filter.repos.contains("/repo-a"));
}

#[test]
fn load_filter_preset_skips_stale_paths() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    // Preset contains a path that no longer exists in repo_paths
    let preset_repos: HashSet<String> = ["/repo-a".to_string(), "/gone".to_string()]
        .into_iter()
        .collect();
    app.filter.presets = vec![("stale".to_string(), preset_repos, RepoFilterMode::Include)];

    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::LoadPreset("stale".to_string()),
    ));
    assert!(app.filter.repos.contains("/repo-a"));
    assert!(
        !app.filter.repos.contains("/gone"),
        "Stale path should be excluded"
    );
}

#[test]
fn start_delete_preset_with_no_presets_is_noop() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::StartDeletePreset,
    ));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn input_preset_name_enter_saves() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-a".to_string()];
    app.filter.repos.insert("/repo-a".to_string());
    app.input.mode = InputMode::InputPresetName;
    app.input.set_buffer("mypreset".to_string());
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert_eq!(app.filter.presets.len(), 1);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::RepoFilter(crate::tui::commands::RepoFilterCommand::PersistFilterPreset { .. })
    )));
}

#[test]
fn input_preset_name_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.input.set_buffer("draft".to_string());
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn input_preset_name_typing_works() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('b')));
    assert_eq!(app.input.buffer, "ab");
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "a");
}

#[test]
fn confirm_delete_preset_letter_deletes() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    app.filter.presets = vec![("alpha".to_string(), repos, RepoFilterMode::Include)];
    app.input.mode = InputMode::ConfirmDeletePreset;
    let cmds = app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
    assert!(app.filter.presets.is_empty());
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::RepoFilter(crate::tui::commands::RepoFilterCommand::DeleteFilterPreset(
            _
        ))
    )));
}

#[test]
fn confirm_delete_preset_esc_cancels() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    app.filter.presets = vec![("alpha".to_string(), repos, RepoFilterMode::Include)];
    app.input.mode = InputMode::ConfirmDeletePreset;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
    assert_eq!(app.filter.presets.len(), 1);
}

#[test]
fn confirm_delete_preset_out_of_range_ignored() {
    let mut app = make_app();
    let repos: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    app.filter.presets = vec![("alpha".to_string(), repos, RepoFilterMode::Include)];
    app.input.mode = InputMode::ConfirmDeletePreset;
    app.handle_key(KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT));
    assert_eq!(app.input.mode, InputMode::ConfirmDeletePreset);
    assert_eq!(app.filter.presets.len(), 1);
}

#[test]
fn render_input_form_shows_during_input_tag() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "My task".to_string(),
        ..Default::default()
    });

    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "New Task"),
        "form overlay title should be visible"
    );
    assert!(
        buffer_contains(&buf, "My task"),
        "draft title should be shown as completed"
    );
    assert!(
        buffer_contains(&buf, "[b]ug"),
        "tag options should be visible"
    );
}

#[test]
fn handle_key_input_preset_name_enter_saves() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::InputPresetName;
    app.input.set_buffer("my-preset".to_string());

    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::RepoFilter(crate::tui::commands::RepoFilterCommand::PersistFilterPreset { .. })
    )));
}

#[test]
fn handle_key_input_preset_name_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::RepoFilter);
}

#[test]
fn handle_key_confirm_delete_preset_selects() {
    let mut app = make_app();
    app.filter.presets = vec![(
        "preset-a".to_string(),
        std::collections::HashSet::new(),
        RepoFilterMode::Include,
    )];
    app.input.mode = InputMode::ConfirmDeletePreset;

    let cmds = app.handle_key(make_key(KeyCode::Char('A')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::RepoFilter(crate::tui::commands::RepoFilterCommand::DeleteFilterPreset(
            _
        ))
    )));
}

#[test]
fn handle_key_confirm_delete_preset_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDeletePreset;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::RepoFilter);
}

#[test]
fn render_status_bar_input_title() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTitle;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Creating task: enter title"),
        "InputTitle mode should show 'Creating task: enter title'"
    );
}

#[test]
fn render_status_bar_input_description() {
    let mut app = make_app();
    app.input.mode = InputMode::InputDescription;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Creating task: opening $EDITOR for description"),
        "InputDescription mode should show 'Creating task: opening $EDITOR for description'"
    );
}

#[test]
fn render_status_bar_input_repo_path() {
    let mut app = make_app();
    app.input.mode = InputMode::InputRepoPath;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Creating task: enter repo path"),
        "InputRepoPath mode should show 'Creating task: enter repo path'"
    );
}

#[test]
fn render_status_bar_input_tag() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Tag:"),
        "InputTag mode should show 'Tag:'"
    );
}

#[test]
fn render_status_bar_confirm_retry() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmRetry(TaskId(1));
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Resume"),
        "ConfirmRetry should show 'Resume'"
    );
    assert!(
        buffer_contains(&buf, "Fresh start"),
        "ConfirmRetry should show 'Fresh start'"
    );
}

#[test]
fn render_status_bar_confirm_detach_tmux() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDetachTmux(vec![TaskId(1)]);
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Detach tmux"),
        "ConfirmDetachTmux should show 'Detach tmux'"
    );
}

#[test]
fn render_status_bar_help_mode() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "[Esc] to close help"),
        "Help mode should show '[Esc] to close help'"
    );
}

#[test]
fn render_status_bar_quick_dispatch() {
    let mut app = make_app();
    app.input.mode = InputMode::QuickDispatch;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Quick dispatch"),
        "QuickDispatch mode should show 'Quick dispatch'"
    );
}

#[test]
fn render_status_bar_input_preset_name() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Enter preset name"),
        "InputPresetName mode should show 'Enter preset name'"
    );
}

#[test]
fn render_status_bar_confirm_delete_preset() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDeletePreset;
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "delete preset"),
        "ConfirmDeletePreset should show 'delete preset'"
    );
}

#[test]
fn render_status_bar_status_message_overrides() {
    let mut app = make_app();
    app.status.message = Some("Custom status message".to_string());
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Custom status message"),
        "status_message should override normal status bar text"
    );
}

#[test]
fn render_input_form_title_shows_new_task_block() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTitle;
    app.input.set_buffer("My new task".to_string());
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "New Task"),
        "block title 'New Task' should be visible"
    );
    assert!(
        buffer_contains(&buf, "Title:"),
        "'Title:' label should be visible"
    );
    assert!(
        buffer_contains(&buf, "My new task"),
        "buffer text 'My new task' should be visible"
    );
}

#[test]
fn render_input_form_description_shows_completed_title() {
    let mut app = make_app();
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft {
        title: "Draft title".to_string(),
        ..Default::default()
    });
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Draft title"),
        "completed title 'Draft title' should be visible"
    );
    assert!(
        buffer_contains(&buf, "Description: opening $EDITOR"),
        "'Description: opening $EDITOR...' should be visible"
    );
}

#[test]
fn render_input_form_base_branch_shows_prompt() {
    let mut app = make_app();
    app.input.mode = InputMode::InputBaseBranch;
    app.input.task_draft = Some(TaskDraft {
        title: "My task".to_string(),
        description: "Desc".to_string(),
        repo_path: "/tmp".to_string(),
        base_branch: "main".into(),
        ..Default::default()
    });
    app.input.set_buffer("main".to_string());
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Base branch:"),
        "'Base branch:' label should be visible"
    );
    assert!(
        buffer_contains(&buf, "main"),
        "pre-filled branch 'main' should be visible"
    );
}

#[test]
fn render_input_form_repo_path_shows_repo_list() {
    let mut app = make_app();
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "Test task".to_string(),
        description: "Test desc".to_string(),
        ..Default::default()
    });
    app.input.set_buffer(String::new());
    app.board.repo_paths = vec!["/repo/alpha".to_string(), "/repo/beta".to_string()];
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Repo path:"),
        "'Repo path:' label should be visible"
    );
    assert!(
        buffer_contains(&buf, "/repo/alpha"),
        "first repo path '/repo/alpha' should be listed"
    );
    assert!(
        buffer_contains(&buf, "/repo/beta"),
        "second repo path '/repo/beta' should be listed"
    );
}

#[test]
fn render_input_form_quick_dispatch_shows_repo_selection() {
    let mut app = make_app();
    app.input.mode = InputMode::QuickDispatch;
    app.board.repo_paths = vec!["/repo/one".to_string(), "/repo/two".to_string()];
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Quick Dispatch"),
        "block title 'Quick Dispatch' should be visible"
    );
    assert!(
        buffer_contains(&buf, "/repo/one"),
        "first repo path '/repo/one' should be visible"
    );
}

#[test]
fn render_input_form_confirm_retry_shows_options() {
    let mut app = make_app();
    // Replace task 5 as a crashed Running task with worktree and tmux
    let now = chrono::Utc::now();
    let crashed_task = Task {
        id: TaskId(5),
        title: "Crashed task".to_string(),
        description: String::new(),
        repo_path: "/repo".to_string(),
        status: TaskStatus::Running,
        worktree: Some("/tmp/wt".to_string()),
        tmux_window: Some("win5".to_string()),
        plan_path: None,
        epic_id: None,
        sub_status: SubStatus::Crashed,
        url: None,
        tag: None,
        sort_order: None,
        base_branch: "main".into(),
        external_id: None,
        labels: Vec::new(),
        created_at: now,
        updated_at: now,
        last_pre_tool_use_at: None,
        last_notification_at: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    };
    app.board.tasks.push(crashed_task);
    app.input.mode = InputMode::ConfirmRetry(TaskId(5));
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Retry Agent"),
        "block title 'Retry Agent' should be visible"
    );
    assert!(
        buffer_contains(&buf, "crashed"),
        "'crashed' label should be visible"
    );
    assert!(
        buffer_contains(&buf, "Resume"),
        "'Resume' option should be visible"
    );
    assert!(
        buffer_contains(&buf, "Fresh start"),
        "'Fresh start' option should be visible"
    );
}

#[test]
fn render_tab_bar_board_mode_has_no_tasks_label() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        !buffer_contains(&buf, "Tasks"),
        "tab bar should not show a 'Tasks' tab label"
    );
}

#[test]
fn status_bar_key_color_is_consistent_across_columns() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
        make_task(3, TaskStatus::Review),
        make_task(4, TaskStatus::Done),
    ]);

    let width = 160;
    let height = 30;
    let status_row = height - 1;

    // Collect the key color from the status bar for each column
    let mut colors = Vec::new();
    for _ in 0..4 {
        let buf = render_to_buffer(&mut app, width, status_row + 1);
        if let Some(color) = first_bracket_fg(&buf, status_row) {
            colors.push(color);
        }
        // Move to next column
        app.handle_key(make_key(KeyCode::Right));
    }

    assert!(
        colors.len() >= 2,
        "should have rendered hints in at least 2 columns"
    );
    let first = colors[0];
    for (i, color) in colors.iter().enumerate() {
        assert_eq!(
            *color, first,
            "column {i} key color {color:?} differs from column 0 color {first:?}"
        );
    }
}

#[test]
fn handle_key_input_preset_name_char() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.input.buffer.clear();

    app.handle_key(make_key(KeyCode::Char('a')));
    assert_eq!(app.input.buffer, "a");
}

#[test]
fn handle_key_input_preset_name_backspace() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    app.input.set_buffer("ab".to_string());

    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "a");
}

#[test]
fn handle_key_input_preset_name_unknown_key_is_noop() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    let cmds = app.handle_key(make_key(KeyCode::Tab));
    assert!(cmds.is_empty());
}

/// InputPresetName mode routes to the preset name handler.
#[test]
fn handle_key_input_preset_name_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::InputPresetName;
    // Esc cancels preset input, returns to RepoFilter
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

/// ConfirmDeletePreset mode routes correctly.
#[test]
fn handle_key_confirm_delete_preset_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDeletePreset;
    // Esc cancels, returns to RepoFilter
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::RepoFilter);
}

#[test]
fn terminal_resized_returns_no_commands() {
    let mut app = make_app();
    let cmds = app.update(Message::System(
        crate::tui::messages::SystemMessage::TerminalResized,
    ));
    assert!(
        cmds.is_empty(),
        "resize should produce no commands, just trigger a re-draw"
    );
}

#[test]
fn show_tips_sets_overlay() {
    let mut app = App::new(vec![]);
    let tips = make_tips();
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Show {
        tips: tips.clone(),
        starting_index: 1,
        max_seen_id: 0,
        show_mode: crate::models::TipsShowMode::Always,
    }));
    let overlay = app.tips.as_ref().expect("tips overlay should be set");
    assert_eq!(overlay.index, 1);
    assert_eq!(overlay.tips.len(), 3);
}

#[test]
fn next_tip_increments_index() {
    let mut app = App::new(vec![]);
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Show {
        tips: make_tips(),
        starting_index: 0,
        max_seen_id: 0,
        show_mode: crate::models::TipsShowMode::Always,
    }));
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Next));
    assert_eq!(app.tips.as_ref().unwrap().index, 1);
}

#[test]
fn next_tip_wraps_at_end() {
    let mut app = App::new(vec![]);
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Show {
        tips: make_tips(),
        starting_index: 2,
        max_seen_id: 0,
        show_mode: crate::models::TipsShowMode::Always,
    }));
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Next));
    assert_eq!(app.tips.as_ref().unwrap().index, 0);
}

#[test]
fn prev_tip_decrements_index() {
    let mut app = App::new(vec![]);
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Show {
        tips: make_tips(),
        starting_index: 2,
        max_seen_id: 0,
        show_mode: crate::models::TipsShowMode::Always,
    }));
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Prev));
    assert_eq!(app.tips.as_ref().unwrap().index, 1);
}

#[test]
fn prev_tip_wraps_at_start() {
    let mut app = App::new(vec![]);
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Show {
        tips: make_tips(),
        starting_index: 0,
        max_seen_id: 0,
        show_mode: crate::models::TipsShowMode::Always,
    }));
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Prev));
    assert_eq!(app.tips.as_ref().unwrap().index, 2);
}

#[test]
fn set_tips_mode_updates_show_mode() {
    let mut app = App::new(vec![]);
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Show {
        tips: make_tips(),
        starting_index: 0,
        max_seen_id: 0,
        show_mode: crate::models::TipsShowMode::Always,
    }));
    app.update(Message::Tips(crate::tui::messages::TipsMessage::SetMode(
        crate::models::TipsShowMode::NewOnly,
    )));
    assert_eq!(
        app.tips.as_ref().unwrap().show_mode,
        crate::models::TipsShowMode::NewOnly
    );
}

#[test]
fn close_tips_clears_overlay_and_returns_save_command() {
    let mut app = App::new(vec![]);
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Show {
        tips: make_tips(),
        starting_index: 1, // tip id=2
        max_seen_id: 0,
        show_mode: crate::models::TipsShowMode::Always,
    }));
    let cmds = app.update(Message::Tips(crate::tui::messages::TipsMessage::Close));
    assert!(app.tips.is_none());
    let save_cmd = cmds.iter().find_map(|c| {
        if let Command::Tips(crate::tui::commands::TipsCommand::SaveState {
            seen_up_to,
            show_mode,
        }) = c
        {
            Some((*seen_up_to, *show_mode))
        } else {
            None
        }
    });
    assert!(
        save_cmd.is_some(),
        "CloseTips should return Tips(SaveState) command"
    );
    let (seen_up_to, _) = save_cmd.unwrap();
    assert_eq!(
        seen_up_to, 2,
        "seen_up_to should be the id of the tip being viewed at close"
    );
}

#[test]
fn close_tips_seen_up_to_respects_max_seen_id() {
    let mut app = App::new(vec![]);
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Show {
        tips: make_tips(),
        starting_index: 0, // tip id=1
        max_seen_id: 5,    // already saw tip 5 previously
        show_mode: crate::models::TipsShowMode::Always,
    }));
    let cmds = app.update(Message::Tips(crate::tui::messages::TipsMessage::Close));
    let seen_up_to = cmds.iter().find_map(|c| {
        if let Command::Tips(crate::tui::commands::TipsCommand::SaveState { seen_up_to, .. }) = c {
            Some(*seen_up_to)
        } else {
            None
        }
    });
    assert_eq!(seen_up_to, Some(5), "seen_up_to should not go backwards");
}

#[test]
fn tips_l_key_goes_next() {
    let mut app = app_with_tips();
    app.handle_key(make_key(KeyCode::Char('l')));
    assert_eq!(app.tips.as_ref().unwrap().index, 2);
}

#[test]
fn tips_right_arrow_goes_next() {
    let mut app = app_with_tips();
    app.handle_key(make_key(KeyCode::Right));
    assert_eq!(app.tips.as_ref().unwrap().index, 2);
}

#[test]
fn tips_h_key_goes_prev() {
    let mut app = app_with_tips();
    app.handle_key(make_key(KeyCode::Char('h')));
    assert_eq!(app.tips.as_ref().unwrap().index, 0);
}

#[test]
fn tips_left_arrow_goes_prev() {
    let mut app = app_with_tips();
    app.handle_key(make_key(KeyCode::Left));
    assert_eq!(app.tips.as_ref().unwrap().index, 0);
}

#[test]
fn tips_n_key_sets_new_only_mode() {
    let mut app = app_with_tips();
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(
        app.tips.as_ref().unwrap().show_mode,
        crate::models::TipsShowMode::NewOnly
    );
    assert!(
        app.status
            .message
            .as_deref()
            .unwrap_or("")
            .contains("Tips:"),
        "n key should emit a Tips status message"
    );
}

#[test]
fn tips_n_key_toggles_back_to_always() {
    let mut app = app_with_tips();
    // First press: Always → NewOnly
    app.handle_key(make_key(KeyCode::Char('n')));
    // Second press: NewOnly → Always
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(
        app.tips.as_ref().unwrap().show_mode,
        crate::models::TipsShowMode::Always
    );
    assert!(
        app.status
            .message
            .as_deref()
            .unwrap_or("")
            .contains("Tips:"),
        "n key should emit a Tips status message when toggling back"
    );
}

#[test]
fn tips_x_key_sets_never_mode() {
    let mut app = app_with_tips();
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(
        app.tips.as_ref().unwrap().show_mode,
        crate::models::TipsShowMode::Never
    );
    assert!(
        app.status
            .message
            .as_deref()
            .unwrap_or("")
            .contains("Tips:"),
        "x key should emit a Tips status message"
    );
}

#[test]
fn tips_q_key_closes_overlay() {
    let mut app = app_with_tips();
    app.handle_key(make_key(KeyCode::Char('q')));
    assert!(app.tips.is_none());
}

#[test]
fn tips_escape_closes_overlay() {
    let mut app = app_with_tips();
    app.handle_key(make_key(KeyCode::Esc));
    assert!(app.tips.is_none());
}

#[test]
fn tips_overlay_captures_input_not_board() {
    // With tips open, pressing 'j' (board navigation) should NOT navigate the board.
    // The overlay captures all input.
    let mut app = app_with_tips();
    let cmds = app.handle_key(make_key(KeyCode::Char('j')));
    // No commands should be emitted for unhandled keys while tips is open
    assert!(cmds.is_empty());
    // Tips overlay is still open
    assert!(app.tips.is_some());
}

#[test]
fn startup_new_only_no_new_tips_returns_none() {
    let tips = vec![make_tip_with_id(1), make_tip_with_id(2)];
    assert!(determine_tips_start(&tips, 2, crate::models::TipsShowMode::NewOnly).is_none());
}

#[test]
fn startup_new_only_with_new_tips_returns_first_new() {
    let tips = vec![
        make_tip_with_id(1),
        make_tip_with_id(2),
        make_tip_with_id(3),
    ];
    let idx = determine_tips_start(&tips, 1, crate::models::TipsShowMode::NewOnly);
    assert_eq!(idx, Some(1)); // tip id=2 is at index 1
}

#[test]
fn startup_always_with_new_tips_returns_first_new() {
    let tips = vec![
        make_tip_with_id(1),
        make_tip_with_id(2),
        make_tip_with_id(3),
    ];
    let idx = determine_tips_start(&tips, 1, crate::models::TipsShowMode::Always);
    assert_eq!(idx, Some(1));
}

#[test]
fn startup_always_no_new_tips_returns_some_index() {
    let tips = vec![make_tip_with_id(1), make_tip_with_id(2)];
    let idx = determine_tips_start(&tips, 5, crate::models::TipsShowMode::Always);
    assert!(idx.is_some());
    assert!(idx.unwrap() < tips.len());
}

#[test]
fn startup_always_empty_tips_returns_none() {
    assert!(determine_tips_start(&[], 0, crate::models::TipsShowMode::Always).is_none());
}

// ---------------------------------------------------------------------------
// Sticky dispatching status (Task #500)
// ---------------------------------------------------------------------------

#[test]
fn set_status_sticky_overrides_existing_status() {
    let mut app = make_app();
    app.set_status("plain message".to_string());
    assert!(!app.status.message_sticky);

    app.set_status_sticky("sticky message".to_string());
    assert_eq!(app.status.message.as_deref(), Some("sticky message"));
    assert!(app.status.message_sticky);
}

#[test]
fn clear_status_resets_message_sticky() {
    let mut app = make_app();
    app.set_status_sticky("sticky".to_string());
    assert!(app.status.message_sticky);

    app.clear_status();
    assert!(app.status.message.is_none());
    assert!(!app.status.message_sticky);
}

#[test]
fn dispatching_status_message_set_on_mark_dispatching() {
    let mut app = make_app(); // task 1 has title "Task 1"
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));

    let msg = app.status.message.as_deref().expect("status set");
    assert!(msg.contains("Task 1"), "got: {msg}");
    assert!(msg.contains("Dispatching"), "got: {msg}");
    assert!(app.status.message_sticky);
}

#[test]
fn dispatching_status_does_not_expire_on_tick() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    // Backdate so a non-sticky message would auto-clear
    app.status.message_set_at = Some(Instant::now() - Duration::from_secs(10));

    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert!(
        app.status.message.is_some(),
        "sticky dispatching status should survive Tick"
    );
}

#[test]
fn dispatching_status_cleared_on_dispatched() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    assert!(app.status.message.is_some());

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::Dispatched {
            id: TaskId(1),
            worktree: "/wt".to_string(),
            tmux_window: "win-1".to_string(),
            switch_focus: false,
        },
    ));

    assert!(app.status.message.is_none());
    assert!(!app.status.message_sticky);
}

#[test]
fn dispatching_status_cleared_on_dispatch_failed() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    assert!(app.status.message.is_some());

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::DispatchFailed(TaskId(1)),
    ));

    assert!(app.status.message.is_none());
    assert!(!app.status.message_sticky);
}

#[test]
fn dispatching_status_pluralizes_when_multiple() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(2)),
    ));

    let msg = app.status.message.as_deref().expect("status set");
    assert!(msg.contains("2 tasks"), "got: {msg}");
    assert!(msg.contains("Dispatching"), "got: {msg}");
}

#[test]
fn dispatching_status_persists_when_one_completes() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(2)),
    ));
    assert!(app.status.message.as_deref().unwrap().contains("2 tasks"));

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::Dispatched {
            id: TaskId(1),
            worktree: "/wt".to_string(),
            tmux_window: "win-1".to_string(),
            switch_focus: false,
        },
    ));

    let msg = app.status.message.as_deref().expect("still set");
    assert!(msg.contains("Task 2"), "got: {msg}");
    assert!(app.status.message_sticky);
}

#[test]
fn dispatching_status_handles_empty_title() {
    let mut app = make_app();
    if let Some(t) = app.find_task_mut(TaskId(1)) {
        t.title = "   ".to_string();
    }

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));

    let msg = app.status.message.as_deref().expect("status set");
    assert!(msg.contains("#1"), "expected ID fallback, got: {msg}");
    assert!(msg.contains("Dispatching"), "got: {msg}");
}

#[test]
fn dispatching_status_skips_deleted_task() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    // Task is deleted while dispatching is in flight.
    app.board.tasks.retain(|t| t.id != TaskId(1));

    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));

    assert!(
        !app.dispatching.contains_key(&TaskId(1)),
        "Tick should drop dispatching IDs that no longer exist in tasks"
    );
    assert!(
        app.status.message.is_none(),
        "with no remaining dispatching IDs, sticky status should clear"
    );
}

#[test]
fn mark_dispatching_for_unknown_task_id_is_noop() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(9999)),
    ));

    // Unknown ID should not pollute the dispatching set with a phantom entry
    // that the next Tick would have to clean up.
    assert!(
        !app.dispatching.contains_key(&TaskId(9999)),
        "MarkDispatching for unknown task should be a no-op"
    );
    assert!(app.status.message.is_none());
}

mod property_tests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum DispatchOp {
        Mark(i64),
        Done(i64),
        Failed(i64),
    }

    fn op_strategy() -> impl Strategy<Value = DispatchOp> {
        prop_oneof![
            (1i64..=4).prop_map(DispatchOp::Mark),
            (1i64..=4).prop_map(DispatchOp::Done),
            (1i64..=4).prop_map(DispatchOp::Failed),
        ]
    }

    proptest! {
        #[test]
        fn sticky_status_iff_dispatching_nonempty(
            ops in proptest::collection::vec(op_strategy(), 0..30)
        ) {
            let mut app = make_app();
            for op in ops {
                match op {
                    DispatchOp::Mark(id) => {
                        app.update(Message::Task(crate::tui::messages::TaskMessage::MarkDispatching(TaskId(id))));
                    }
                    DispatchOp::Done(id) => {
                        app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatched {
                            id: TaskId(id),
                            worktree: format!("/wt/{id}"),
                            tmux_window: format!("win-{id}"),
                            switch_focus: false,
                        }));
                    }
                    DispatchOp::Failed(id) => {
                        app.update(Message::Task(crate::tui::messages::TaskMessage::DispatchFailed(TaskId(id))));
                    }
                }
            }

            // Invariant: status is sticky exactly when dispatching is non-empty
            prop_assert_eq!(
                app.dispatching.is_empty(),
                !app.status.message_sticky,
                "dispatching set membership and sticky-flag must agree"
            );
            // When sticky, a message is present; when not, nothing claims to be sticky
            if app.status.message_sticky {
                prop_assert!(app.status.message.is_some());
            }
        }
    }
}
