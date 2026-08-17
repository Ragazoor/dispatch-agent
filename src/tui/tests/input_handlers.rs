#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{EpicId, SubStatus, TaskId, TaskStatus, TaskTag};
use crate::tui::commands::SettingsCommand;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn task_created_adds_to_list() {
    let now = chrono::Utc::now();
    let task = Task {
        id: TaskId(42),
        title: "New Task".to_string(),
        description: "desc".to_string(),
        repo_path: "/repo".to_string(),
        status: TaskStatus::Backlog,
        worktree: None,
        tmux_window: None,
        plan_path: None,
        epic_id: None,
        sub_status: SubStatus::None,
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
        last_peer_message_sent_at: None,
        last_peer_message_received_at: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        live_subagents: 0,
        stop_pending: false,
        live_shells: 0,
        oldest_live_shell_started_at: None,
        schedule_interval_secs: None,
        pinned_branch: None,
        last_processed_sha: None,
        last_scheduled_check_at: None,
    };
    let mut app = App::new(vec![]);
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Created {
        task: Box::new(task),
    }));
    assert_eq!(app.board.tasks.len(), 1);
    assert_eq!(app.board.tasks[0].id, TaskId(42));
    assert_eq!(app.board.tasks[0].status, TaskStatus::Backlog);
    assert!(cmds.is_empty());
}

#[test]
fn repo_path_empty_uses_saved_path() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp".to_string()];

    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        description: "desc".to_string(),
        ..Default::default()
    });
    app.input.buffer.clear();

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    // Now advances to InputBaseBranch with "main" pre-filled
    assert_eq!(app.input.mode, InputMode::InputBaseBranch);
    assert_eq!(app.input.buffer, "main");
    assert!(cmds.is_empty());
    // Submitting base branch goes to wrap-up mode
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitBaseBranch("main".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputWrapUpMode);
    // Skipping wrap-up drops into the schedule gate; declining that creates
    // the task (tasks.allium: CreateTask, "The schedule step").
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitWrapUpMode(None),
    ));
    assert_eq!(app.input.mode, InputMode::InputScheduleGate);
    let cmds3 = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitScheduleGate(false),
    ));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds3.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Insert { ref draft, .. }) if draft.repo_path == "/tmp"
    )));
}

#[test]
fn repo_path_empty_no_saved_stays_in_mode() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec![]; // no saved paths

    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        description: "desc".to_string(),
        ..Default::default()
    });
    app.input.buffer.clear();

    let key = make_key(KeyCode::Enter);
    let _cmds = app.handle_key(key);

    // Should stay in InputRepoPath mode
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert!(app.status.message.is_some());
    assert_eq!(app.board.tasks.len(), 0); // no task created
}

#[test]
fn repo_path_nonexistent_shows_error() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: "D".to_string(),
        ..Default::default()
    });
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitRepoPath("/nonexistent/path".to_string()),
    ));
    assert!(cmds.is_empty());
    assert!(app.status.message.is_some());
    let msg = app.status.message.as_ref().unwrap().as_str();
    assert!(msg.contains("does not exist"), "got: {msg}");
}

#[test]
fn repo_path_nonempty_used_as_is() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp".to_string()];

    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        description: "desc".to_string(),
        ..Default::default()
    });
    app.input.set_buffer("/tmp".to_string());

    // Submitting repo path now advances to InputBaseBranch
    let cmds = without_usage(app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert_eq!(app.input.mode, InputMode::InputBaseBranch);
    assert_eq!(app.input.buffer, "main");
    assert!(cmds.is_empty());
    // Submitting base branch goes to wrap-up mode
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitBaseBranch("main".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputWrapUpMode);
    // Skipping wrap-up drops into the schedule gate; declining that completes
    // creation (tasks.allium: CreateTask, "The schedule step").
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitWrapUpMode(None),
    ));
    assert_eq!(app.input.mode, InputMode::InputScheduleGate);
    let cmds3 = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitScheduleGate(false),
    ));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds3
        .iter()
        .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Insert { ref draft, .. }) if draft.repo_path == "/tmp")));
    assert_eq!(app.board.tasks.len(), 0); // task not added until TaskCreated
}

#[test]
fn task_edited_updates_fields() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.update(Message::Task(crate::tui::messages::TaskMessage::Edited(
        TaskEdit {
            id: TaskId(1),
            title: "New".into(),
            description: "Desc".into(),
            repo_path: "/new".into(),
            status: TaskStatus::Running,
            plan_path: Some("docs/plan.md".into()),
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            url: None,
            schedule_interval_secs: Some(600),
            pinned_branch: Some("staging".into()),
        },
    )));
    assert_eq!(app.board.tasks[0].title, "New");
    assert_eq!(app.board.tasks[0].description, "Desc");
    assert_eq!(app.board.tasks[0].repo_path, "/new");
    assert_eq!(app.board.tasks[0].status, TaskStatus::Running);
    assert_eq!(
        app.board.tasks[0].plan_path.as_deref(),
        Some("docs/plan.md")
    );
    // The scheduling pair rides the same edit, so the card's scheduled badge
    // appears with the edit rather than at the next DB refresh.
    assert_eq!(app.board.tasks[0].schedule_interval_secs, Some(600));
    assert_eq!(app.board.tasks[0].pinned_branch.as_deref(), Some("staging"));
}

#[test]
fn repo_paths_updated_replaces_paths() {
    let mut app = App::new(vec![]);
    app.update(Message::RepoPathsUpdated(vec!["/a".into(), "/b".into()]));
    assert_eq!(app.board.repo_paths, vec!["/a", "/b"]);
}

#[test]
fn n_key_enters_title_mode() {
    let mut app = make_app();
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::InputTitle);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert_eq!(app.status.message.as_deref(), Some("Enter title: "));
}

#[test]
fn backspace_pops_from_input_buffer() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.input.set_buffer("abc".to_string());
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "ab");
}

#[test]
fn backspace_on_empty_buffer_is_noop() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.input.buffer.clear();
    app.handle_key(make_key(KeyCode::Backspace));
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.mode, InputMode::InputTitle);
}

#[test]
fn enter_with_title_advances_to_tag() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.input.set_buffer("My Task".to_string());
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputTag);
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.task_draft.as_ref().unwrap().title, "My Task");
    assert_eq!(
        app.status.message.as_deref(),
        Some("Tag: [b]ug  [f]eature  [c]hore  [p]r-review  [r]esearch  [x]fix  [Enter] none")
    );
}

#[test]
fn enter_with_empty_title_cancels() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.input.buffer.clear();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
    assert!(app.status.message.is_none());
}

#[test]
fn enter_with_whitespace_only_title_cancels() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.input.set_buffer("   ".to_string());
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
}

#[test]
fn enter_in_description_advances_to_repo_path() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: String::new(),
        ..Default::default()
    });
    app.input.set_buffer("some desc".to_string());
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert!(app.input.buffer.is_empty());
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().description,
        "some desc"
    );
    assert_eq!(app.status.message.as_deref(), Some("Enter repo path: "));
}

#[test]
fn number_key_out_of_range_appends_to_buffer() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: String::new(),
        ..Default::default()
    });
    app.input.buffer.clear();
    app.board.repo_paths = vec!["/repo1".to_string()]; // only 1 path
    app.handle_key(make_key(KeyCode::Char('5')));
    assert_eq!(app.input.buffer, "5");
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
}

#[test]
fn number_key_with_nonempty_buffer_appends() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: String::new(),
        ..Default::default()
    });
    app.input.set_buffer("/my".to_string());
    app.board.repo_paths = vec!["/repo1".to_string()];
    app.handle_key(make_key(KeyCode::Char('1')));
    assert_eq!(app.input.buffer, "/my1");
}

#[test]
fn zero_key_in_repo_path_appends_to_buffer() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: String::new(),
        ..Default::default()
    });
    app.input.buffer.clear();
    app.board.repo_paths = vec!["/repo".to_string()];
    app.handle_key(make_key(KeyCode::Char('0')));
    assert_eq!(app.input.buffer, "0");
}

#[test]
fn escape_from_title_mode_cancels() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.input.set_buffer("partial".to_string());
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert!(app.status.message.is_none());
}

#[test]
fn escape_from_description_mode_cancels() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: String::new(),
        ..Default::default()
    });
    app.input.set_buffer("partial".to_string());
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert!(app.status.message.is_none());
}

#[test]
fn escape_from_repo_path_mode_cancels() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: String::new(),
        ..Default::default()
    });
    app.input.set_buffer("/partial".to_string());
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert!(app.status.message.is_none());
}

#[test]
fn confirm_delete_y_deletes_task() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.input.mode = InputMode::ConfirmDelete;
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.board.tasks.iter().all(|t| t.id != TaskId(1))); // task 1 deleted
    assert!(matches!(
        &cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::Delete(TaskId(1)))
    ));
    assert!(app.status.message.is_none());
}

#[test]
fn confirm_delete_uppercase_y_deletes_task() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.input.mode = InputMode::ConfirmDelete;
    let cmds = app.handle_key(make_key(KeyCode::Char('Y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.board.tasks.iter().all(|t| t.id != TaskId(1)));
    assert!(matches!(
        &cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::Delete(TaskId(1)))
    ));
}

#[test]
fn confirm_delete_n_cancels() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.input.mode = InputMode::ConfirmDelete;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert_eq!(app.board.tasks.len(), 4);
    assert!(cmds.is_empty());
    assert!(app.status.message.is_none());
}

#[test]
fn confirm_delete_esc_cancels() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.input.mode = InputMode::ConfirmDelete;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert_eq!(app.board.tasks.len(), 4);
    assert!(cmds.is_empty());
}

#[test]
fn x_key_on_empty_column_is_noop() {
    let mut app = make_app();
    app.selection_mut().set_column(3); // Review column is empty
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::Normal); // did NOT enter ConfirmArchive
}

#[test]
fn open_close_task_detail_via_messages() {
    let mut app = App::new(vec![]);
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::OpenDetail(TaskId(1)),
    ));
    assert!(matches!(app.board.view_mode, ViewMode::TaskDetail { .. }));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::CloseDetail,
    ));
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn enter_key_on_empty_board_is_noop() {
    let mut app = App::new(vec![]);
    app.handle_key(make_key(KeyCode::Enter));
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

#[test]
fn e_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![]);
    app.selection_mut().set_column(1);
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(cmds.is_empty());
}

#[test]
fn e_key_directly_emits_edit_task() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.selection_mut().set_column(1);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('e'))));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Editor(crate::tui::commands::EditorCommand::PopOut(EditKind::TaskEdit(t))) if t.id == TaskId(1))
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_retry_r_key_emits_resume() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());
    app.board.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.handle_key(make_key(KeyCode::Char('r')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Resume { .. })
    )));
}

#[test]
fn confirm_retry_f_key_emits_fresh() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());
    app.board.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.handle_key(make_key(KeyCode::Char('f')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    )));
}

#[test]
fn confirm_retry_esc_returns_to_normal() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty());
}

#[test]
fn start_new_task_enters_title_mode() {
    let mut app = make_app();
    app.update(Message::Input(
        crate::tui::messages::InputMessage::StartNewTask,
    ));
    assert_eq!(app.input.mode, InputMode::InputTitle);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert_eq!(app.status.message.as_deref(), Some("Enter title: "));
}

#[test]
fn cancel_input_returns_to_normal() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.input.set_buffer("partial".to_string());
    app.input.task_draft = Some(TaskDraft::default());
    app.status.message = Some("Enter title: ".to_string());
    app.update(Message::Input(
        crate::tui::messages::InputMessage::CancelInput,
    ));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert!(app.status.message.is_none());
}

#[test]
fn submit_title_with_text_advances_to_tag() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitTitle("My Task".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputTag);
    assert_eq!(app.input.task_draft.as_ref().unwrap().title, "My Task");
    assert_eq!(
        app.status.message.as_deref(),
        Some("Tag: [b]ug  [f]eature  [c]hore  [p]r-review  [r]esearch  [x]fix  [Enter] none")
    );
}

#[test]
fn submit_empty_title_cancels() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTitle;
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitTitle(String::new()),
    ));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
}

#[test]
fn submit_tag_advances_to_description() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitTag(Some(TaskTag::Bug)),
    ));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(
        &cmds[0],
        Command::Editor(crate::tui::commands::EditorCommand::PopOut(
            EditKind::Description { is_epic: false }
        ))
    ));
    assert_eq!(app.input.mode, InputMode::InputDescription);
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().tag,
        Some(TaskTag::Bug)
    );
    assert_eq!(
        app.status.message.as_deref(),
        Some("Opening editor for description...")
    );
}

#[test]
fn submit_description_advances_to_repo_path() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitDescription("my desc".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().description,
        "my desc"
    );
}

#[test]
fn description_editor_result_advances_to_repo_path() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    app.update(Message::Editor(
        crate::tui::messages::EditorMessage::DescriptionResult("some desc".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().description,
        "some desc"
    );
}

#[test]
fn description_editor_result_multiline() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    app.update(Message::Editor(
        crate::tui::messages::EditorMessage::DescriptionResult("Line 1\nLine 2".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().description,
        "Line 1\nLine 2"
    );
}

#[test]
fn editor_result_description_saved_advances_draft() {
    // EditorResult{Description, Saved(raw)} must parse sections out of the
    // raw editor output and feed the description into the existing
    // DescriptionEditorResult flow.
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    app.update(Message::Editor(
        crate::tui::messages::EditorMessage::Result {
            kind: EditKind::Description { is_epic: false },
            outcome: EditorOutcome::Saved("--- DESCRIPTION ---\nhello from editor\n".to_string()),
        },
    ));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().description,
        "hello from editor"
    );
}

#[test]
fn editor_result_description_cancelled_cancels_input() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    app.update(Message::Editor(
        crate::tui::messages::EditorMessage::Result {
            kind: EditKind::Description { is_epic: false },
            outcome: EditorOutcome::Cancelled,
        },
    ));
    // Cancelling during description input returns to Normal mode.
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn editor_result_task_edit_returns_finalize_command() {
    // Non-description EditKind variants route through a FinalizeEditorResult
    // command so the runtime applies the edit via services.
    use crate::models::{Task, TaskId, TaskStatus};
    let task = Task {
        id: TaskId(42),
        title: "t".into(),
        description: "d".into(),
        repo_path: "/r".into(),
        status: TaskStatus::Backlog,
        worktree: None,
        tmux_window: None,
        plan_path: None,
        epic_id: None,
        sub_status: crate::models::SubStatus::None,
        url: None,
        tag: None,
        sort_order: None,
        base_branch: "main".into(),
        external_id: None,
        labels: Vec::new(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_pre_tool_use_at: None,
        last_notification_at: None,
        last_peer_message_sent_at: None,
        last_peer_message_received_at: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        live_subagents: 0,
        stop_pending: false,
        live_shells: 0,
        oldest_live_shell_started_at: None,
        schedule_interval_secs: None,
        pinned_branch: None,
        last_processed_sha: None,
        last_scheduled_check_at: None,
    };
    let mut app = App::new(vec![task.clone()]);
    let cmds = app.update(Message::Editor(
        crate::tui::messages::EditorMessage::Result {
            kind: EditKind::TaskEdit(Box::new(task)),
            outcome: EditorOutcome::Saved("--- TITLE ---\nNew\n".into()),
        },
    ));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Editor(crate::tui::commands::EditorCommand::FinalizeResult {
                kind: EditKind::TaskEdit(t),
                outcome: EditorOutcome::Saved(_),
            }) if t.id == TaskId(42)
        )),
        "expected FinalizeEditorResult(TaskEdit(42)), got {:?}",
        cmds
    );
}

#[test]
fn submit_repo_path_advances_to_base_branch() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: "D".to_string(),
        tag: Some(TaskTag::Bug),
        ..Default::default()
    });
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitRepoPath("/tmp".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputBaseBranch);
    assert_eq!(app.input.buffer, "main");
    assert!(cmds.is_empty());
}

#[test]
fn submit_base_branch_sets_branch_and_advances_to_wrap_up_mode() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputBaseBranch;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: "D".to_string(),
        repo_path: "/tmp".to_string(),
        tag: Some(TaskTag::Bug),
        base_branch: "main".into(),
        ..Default::default()
    });
    app.input.set_buffer("develop".to_string());
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitBaseBranch("develop".to_string()),
    ));
    // Now transitions to wrap-up mode selection instead of creating the task directly.
    assert_eq!(app.input.mode, InputMode::InputWrapUpMode);
    assert!(
        cmds.is_empty(),
        "no Insert yet — wrap-up mode selection is next"
    );
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().base_branch,
        "develop"
    );
}

#[test]
fn submit_base_branch_empty_uses_draft_default() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::InputBaseBranch;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: "D".to_string(),
        repo_path: "/tmp".to_string(),
        base_branch: "main".into(),
        ..Default::default()
    });
    app.input.set_buffer(String::new());
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitBaseBranch(String::new()),
    ));
    assert_eq!(app.input.mode, InputMode::InputWrapUpMode);
    assert!(
        cmds.is_empty(),
        "no Insert yet — wrap-up mode selection is next"
    );
    assert_eq!(app.input.task_draft.as_ref().unwrap().base_branch, "main");
}

// ---------------------------------------------------------------------------
// BaseBranchPicker (task #3422) — see docs/specs/dispatch.allium: surface
// BaseBranchPicker, rule RecordBaseBranch. Mirrors the InputRepoPath /
// RepoPathPicker tests above.
// ---------------------------------------------------------------------------

#[test]
fn submit_repo_path_prefills_base_branch_from_history() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp".to_string()];
    app.board.repo_base_branches = std::collections::HashMap::from([(
        "/tmp".to_string(),
        vec!["develop".to_string(), "main".to_string()],
    )]);
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: "D".to_string(),
        ..Default::default()
    });
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitRepoPath("/tmp".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputBaseBranch);
    assert_eq!(
        app.input.buffer, "develop",
        "prefill should be the most-recently-used branch for /tmp (PrefillFromHistory)"
    );
    assert!(cmds.is_empty());
}

#[test]
fn submit_repo_path_no_history_falls_back_to_default() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp".to_string()];
    // No entry in repo_base_branches for "/tmp".
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: "D".to_string(),
        ..Default::default()
    });
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitRepoPath("/tmp".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputBaseBranch);
    assert_eq!(
        app.input.buffer, "main",
        "falls back to the draft default when the repo has no branch history"
    );
    assert!(cmds.is_empty());
}

#[test]
fn typing_in_base_branch_fuzzy_filters_history_and_resets_cursor() {
    let mut app = App::new(vec![]);
    app.board.repo_base_branches = std::collections::HashMap::from([(
        "/tmp".to_string(),
        vec![
            "develop".to_string(),
            "main".to_string(),
            "release".to_string(),
        ],
    )]);
    app.input.mode = InputMode::InputBaseBranch;
    app.input.task_draft = Some(TaskDraft {
        repo_path: "/tmp".to_string(),
        ..Default::default()
    });
    app.input.set_buffer(String::new());
    app.input.repo_cursor = 2; // simulate a prior cursor position

    app.handle_key(make_key(KeyCode::Char('d')));
    app.handle_key(make_key(KeyCode::Char('e')));
    app.handle_key(make_key(KeyCode::Char('v')));

    assert_eq!(app.input.buffer, "dev");
    assert_eq!(
        app.input.repo_cursor, 0,
        "the list cursor resets to 0 on every query-changing keystroke"
    );
}

#[test]
fn arrow_keys_move_base_branch_cursor_with_wraparound() {
    let mut app = App::new(vec![]);
    app.board.repo_base_branches = std::collections::HashMap::from([(
        "/tmp".to_string(),
        vec!["main".to_string(), "develop".to_string()],
    )]);
    app.input.mode = InputMode::InputBaseBranch;
    app.input.task_draft = Some(TaskDraft {
        repo_path: "/tmp".to_string(),
        ..Default::default()
    });
    app.input.set_buffer(String::new());
    app.input.repo_cursor = 0;

    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 1);
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 0, "cursor wraps from last back to 0");
    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(
        app.input.repo_cursor, 1,
        "cursor wraps from 0 back to the last entry going up"
    );
}

#[test]
fn enter_on_base_branch_list_item_submits_that_branch() {
    let mut app = App::new(vec![]);
    app.board.repo_base_branches = std::collections::HashMap::from([(
        "/tmp".to_string(),
        vec!["develop".to_string(), "main".to_string()],
    )]);
    app.input.mode = InputMode::InputBaseBranch;
    app.input.task_draft = Some(TaskDraft {
        repo_path: "/tmp".to_string(),
        base_branch: "main".into(),
        ..Default::default()
    });
    // "e" fuzzy-matches "develop" but not "main"; cursor 0 highlights "develop".
    app.input.set_buffer("e".to_string());
    app.input.repo_cursor = 0;

    app.handle_key(make_key(KeyCode::Enter));

    assert_eq!(app.input.mode, InputMode::InputWrapUpMode);
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().base_branch,
        "develop",
        "Enter on the highlighted history item should submit the full branch name, not the raw query"
    );
}

#[test]
fn enter_on_new_base_branch_entry_submits_typed_text() {
    let mut app = App::new(vec![]);
    app.board.repo_base_branches =
        std::collections::HashMap::from([("/tmp".to_string(), vec!["develop".to_string()])]);
    app.input.mode = InputMode::InputBaseBranch;
    app.input.task_draft = Some(TaskDraft {
        repo_path: "/tmp".to_string(),
        base_branch: "main".into(),
        ..Default::default()
    });
    // "e" fuzzy-matches "develop", so effective = [develop, NewBranch("e")].
    // Cursor 1 highlights the synthetic new-branch entry, not "develop".
    app.input.set_buffer("e".to_string());
    app.input.repo_cursor = 1;

    app.handle_key(make_key(KeyCode::Enter));

    assert_eq!(app.input.mode, InputMode::InputWrapUpMode);
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().base_branch,
        "e",
        "Enter on the synthetic new-branch entry should submit the typed query verbatim"
    );
}

#[test]
fn cancel_retry_returns_to_normal() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));
    app.status.message = Some("Agent stale".to_string());
    app.update(Message::Input(
        crate::tui::messages::InputMessage::CancelRetry,
    ));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status.message.is_none());
}

#[test]
fn esc_clears_selection() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));
    assert_eq!(app.select.tasks.len(), 2);

    app.handle_key(make_key(KeyCode::Esc));
    assert!(app.select.tasks.is_empty());
}

#[test]
fn esc_with_no_selection_is_noop() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn x_key_with_done_selection_shows_count_in_confirm() {
    // Only an all-Done selection archives — anything short of Done moves
    // to Done instead.
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
    assert_eq!(
        app.status.message.as_deref(),
        Some("Archive 2 items? [y/n]")
    );
}

#[test]
fn x_key_with_selection_shows_count_in_move_to_done_confirm() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(2)),
    ));

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmDone);
    assert_eq!(
        app.status.message.as_deref(),
        Some("Move 2 tasks to Done? [y/n]")
    );
}

#[test]
fn enter_on_task_opens_task_detail() {
    // Enter in Normal mode on a task opens the TaskDetail overlay.
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    app.handle_key(make_key(KeyCode::Enter));
    // Should open task detail for the first task in Backlog column
    assert!(
        matches!(app.board.view_mode, ViewMode::TaskDetail { task_id, .. } if task_id == TaskId(1))
    );
}

#[test]
fn left_arrow_navigates_column() {
    let mut app = make_app();
    app.selection_mut().set_column(2);
    app.handle_key(make_key(KeyCode::Left));
    assert_eq!(app.selection().column(), 1);
}

#[test]
fn right_arrow_navigates_column() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.handle_key(make_key(KeyCode::Right));
    assert_eq!(app.selection().column(), 2);
}

#[test]
fn down_arrow_navigates_row() {
    let mut app = make_app();
    app.selection_mut().set_column(1); // Backlog has 2 tasks
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.selection().row(1), 1);
}

#[test]
fn up_arrow_navigates_row() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1);
    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(app.selection().row(1), 0);
}

#[test]
fn confirm_retry_unrecognized_key_is_noop() {
    let mut app = App::new(vec![]);
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));
    let cmds = app.handle_key(make_key(KeyCode::Char('x')));
    assert!(cmds.is_empty());
    assert!(matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn esc_dismisses_help() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;

    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn esc_clears_selection_and_exits_toggle() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('k')));
    assert!(app.on_select_all());
    app.handle_key(make_key(KeyCode::Esc));
    assert!(app.select.tasks.is_empty());
    assert!(!app.on_select_all());
}

#[test]
fn handle_key_dismisses_error_popup() {
    let mut app = make_app();
    app.status.error_popup = Some("something went wrong".to_string());
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('q'))));
    assert!(app.status.error_popup.is_none());
    assert!(cmds.is_empty());
}

#[test]
fn handle_key_normal_navigation() {
    let mut app = make_app();
    // Start at column 1 (Backlog), row 0
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    // 'l' moves right
    app.handle_key(make_key(KeyCode::Char('l')));
    assert_eq!(app.selection().column(), 2);

    // 'h' moves left
    app.handle_key(make_key(KeyCode::Char('h')));
    assert_eq!(app.selection().column(), 1);

    // 'j' moves down
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.selection().row(1), 1);

    // 'k' moves up
    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.selection().row(1), 0);
}

#[test]
fn confirm_quit_y_quits() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmQuit;
    app.handle_key(make_key(KeyCode::Char('y')));
    assert!(app.should_quit);
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_quit_n_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmQuit;
    app.handle_key(make_key(KeyCode::Char('n')));
    assert!(!app.should_quit);
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_quit_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmQuit;
    app.handle_key(make_key(KeyCode::Esc));
    assert!(!app.should_quit);
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn handle_key_normal_new_task() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(*app.mode(), InputMode::InputTitle);
}

#[test]
fn handle_key_normal_toggle_help() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('?')));
    assert_eq!(*app.mode(), InputMode::Help);
}

#[test]
fn handle_key_help_dismiss() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;

    // '?' toggles help off
    app.handle_key(make_key(KeyCode::Char('?')));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_help_esc_dismiss() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_text_input_char_and_backspace() {
    let mut app = make_app();
    // Enter title input mode
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(*app.mode(), InputMode::InputTitle);

    // Type characters
    app.handle_key(make_key(KeyCode::Char('H')));
    app.handle_key(make_key(KeyCode::Char('i')));
    assert_eq!(app.input.buffer, "Hi");

    // Backspace removes last char
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "H");
}

#[test]
fn caret_left_right_move_and_clamp() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('b')));
    app.handle_key(make_key(KeyCode::Char('c')));
    assert_eq!(app.input.caret, 3);
    // Left moves toward the start
    app.handle_key(make_key(KeyCode::Left));
    assert_eq!(app.input.caret, 2);
    app.handle_key(make_key(KeyCode::Left));
    app.handle_key(make_key(KeyCode::Left));
    assert_eq!(app.input.caret, 0);
    // Clamp at 0
    app.handle_key(make_key(KeyCode::Left));
    assert_eq!(app.input.caret, 0);
    // Right moves back, clamped at len
    app.handle_key(make_key(KeyCode::Right));
    assert_eq!(app.input.caret, 1);
    app.handle_key(make_key(KeyCode::End));
    assert_eq!(app.input.caret, 3);
    app.handle_key(make_key(KeyCode::Right));
    assert_eq!(app.input.caret, 3);
}

#[test]
fn caret_insert_and_backspace_mid_buffer() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('c')));
    // Move between a and c, then insert b
    app.handle_key(make_key(KeyCode::Left));
    app.handle_key(make_key(KeyCode::Char('b')));
    assert_eq!(app.input.buffer, "abc");
    assert_eq!(app.input.caret, 2);
    // Backspace deletes the char before the caret ('b'), not the last ('c')
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "ac");
    assert_eq!(app.input.caret, 1);
}

#[test]
fn caret_delete_forward_removes_char_at_caret() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('b')));
    app.handle_key(make_key(KeyCode::Home));
    assert_eq!(app.input.caret, 0);
    app.handle_key(make_key(KeyCode::Delete));
    assert_eq!(app.input.buffer, "b");
    assert_eq!(app.input.caret, 0);
}

#[test]
fn caret_word_jump_ctrl_arrows() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    type_text(&mut app, "foo bar baz");
    assert_eq!(app.input.caret, 11);
    // Ctrl+Left jumps to the start of the last word ("baz")
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.input.caret, 8);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.input.caret, 4);
    // Ctrl+Right jumps forward one word
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(app.input.caret, 8);
}

#[test]
fn caret_word_jump_alt_bf_fallback() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    type_text(&mut app, "foo bar");
    assert_eq!(app.input.caret, 7);
    // Alt+B is the readline word-left fallback (tmux without xterm-keys)
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
    assert_eq!(app.input.caret, 4);
    // Alt+F word-right
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
    assert_eq!(app.input.caret, 7);
    // A plain 'b' with no modifier still types into the buffer
    app.handle_key(make_key(KeyCode::Char('b')));
    assert_eq!(app.input.buffer, "foo barb");
}

#[test]
fn every_handled_key_marks_dirty_including_true_noops() {
    // handle_key always marks the frame dirty, even for keys that produce no
    // visible change (e.g. caret already at the boundary). Computing which
    // fields changed per-handler proved fragile in practice (see
    // docs/architecture.md's dirty-flag section) — every mutating handler
    // would need to remember to opt in, and several forgot to. The 16ms
    // frame-rate cap in `frame_ready` already bounds the cost of redrawing on
    // a true no-op, so there is no correctness/perf reason to skip it.
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Left)); // caret 0
    app.dirty = false;
    app.handle_key(make_key(KeyCode::Right)); // caret 1 -> visible move
    assert!(app.dirty, "a real caret move must mark the frame dirty");

    // Now a true no-op: Left at caret 0 — still marks dirty.
    app.handle_key(make_key(KeyCode::Home)); // caret 0
    app.dirty = false;
    app.handle_key(make_key(KeyCode::Left)); // stays at 0
    assert!(
        app.dirty,
        "handle_key must mark dirty unconditionally, even for a no-op caret move"
    );
}

#[test]
fn caret_prefilled_todo_edit_lands_at_end() {
    let mut app = make_app();
    app.input.set_buffer("existing".to_string());
    assert_eq!(app.input.caret, "existing".chars().count());
}

#[test]
fn caret_repo_mode_left_right_moves_caret_not_list() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.clear_buffer();
    // "/" is a subsequence of every saved path, so all three stay in the list.
    app.handle_key(make_key(KeyCode::Char('/')));
    assert_eq!(app.input.caret, 1);
    // Down moves the repo list cursor (list nav, not text caret)
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 1);
    // Left moves the text caret, not the list cursor
    app.handle_key(make_key(KeyCode::Left));
    assert_eq!(app.input.caret, 0);
    assert_eq!(app.input.repo_cursor, 1);
    // Typing resets the list cursor to 0 and inserts at the caret
    app.handle_key(make_key(KeyCode::Char('a')));
    assert_eq!(app.input.repo_cursor, 0);
    assert_eq!(app.input.buffer, "a/");
}

#[test]
fn handle_key_text_input_esc_cancels() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(*app.mode(), InputMode::InputTitle);

    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_text_input_enter_advances_to_tag() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('n')));
    app.handle_key(make_key(KeyCode::Char('T')));
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(*app.mode(), InputMode::InputTag);
}

#[test]
fn handle_key_confirm_retry_resume() {
    let mut app = make_app();
    let mut task = make_task(10, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/10-test".to_string());
    task.tmux_window = Some("main:10-test".to_string());
    app.board.tasks.push(task);
    app.input.mode = InputMode::ConfirmRetry(TaskId(10));

    let cmds = app.handle_key(make_key(KeyCode::Char('r')));
    // Should produce KillTmuxWindow + Resume
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::KillTmuxWindow { .. })
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Resume { .. })
    )));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_retry_fresh() {
    let mut app = make_app();
    let mut task = make_task(10, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/10-test".to_string());
    task.tmux_window = Some("main:10-test".to_string());
    app.board.tasks.push(task);
    app.input.mode = InputMode::ConfirmRetry(TaskId(10));

    let cmds = app.handle_key(make_key(KeyCode::Char('f')));
    // Should produce Cleanup + Dispatch
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    )));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_confirm_retry_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmRetry(TaskId(10));
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_quick_dispatch_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::QuickDispatch;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_tag_selects_bug() {
    let mut app = make_app();
    // Tag comes right after title, before description/repo
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('b'))));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(
        &cmds[0],
        Command::Editor(crate::tui::commands::EditorCommand::PopOut(
            EditKind::Description { is_epic: false }
        ))
    ));
    assert_eq!(*app.mode(), InputMode::InputDescription);
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().tag,
        Some(TaskTag::Bug)
    );
}

#[test]
fn handle_key_tag_skip_with_enter() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(
        &cmds[0],
        Command::Editor(crate::tui::commands::EditorCommand::PopOut(
            EditKind::Description { is_epic: false }
        ))
    ));
    assert_eq!(*app.mode(), InputMode::InputDescription);
    assert_eq!(app.input.task_draft.as_ref().unwrap().tag, None);
}

#[test]
fn handle_key_tag_esc_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(*app.mode(), InputMode::Normal);
}

#[test]
fn handle_key_normal_activate_backlog_task() {
    let mut app = make_app();
    // Select task 1 (backlog)
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char(' ')));
    // The trust check is deferred to a Command (see CheckTrustAndDispatch) —
    // handle_key must not decide dispatch-vs-confirm-trust synchronously.
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::CheckTrustAndDispatch { .. })
        )),
        "expected CheckTrustAndDispatch command, got {cmds:?}"
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn handle_key_normal_activate_running_task_with_window_jumps() {
    let mut app = make_app();
    // Select running task (column 2)
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);
    // Give running task a window
    let task_3 = app
        .board
        .tasks
        .iter_mut()
        .find(|t| t.id == TaskId(3))
        .unwrap();
    task_3.tmux_window = Some("main:task-3".to_string());

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    // Space jumps to the live window rather than dispatching.
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { window }) if window == "main:task-3"
        )),
        "Space on a running task with a window should jump, got {cmds:?}"
    );
}

#[test]
fn handle_key_normal_enter_opens_task_detail() {
    // Enter key on a task opens the TaskDetail overlay.
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    app.handle_key(make_key(KeyCode::Enter));
    assert!(
        matches!(app.board.view_mode, ViewMode::TaskDetail { task_id, .. } if task_id == TaskId(1))
    );
}

#[test]
fn handle_key_normal_g_starts_pending_chord_without_firing() {
    // A lone `g` no longer fires jump-to-tmux immediately: it starts the
    // `gg`-chord pending state and waits for either a second `g` (jump to
    // top) or a fallback (next key / idle tick) to resolve it.
    let mut app = make_app();
    let task = app
        .board
        .tasks
        .iter_mut()
        .find(|t| t.id == TaskId(3))
        .unwrap();
    task.tmux_window = Some("main:task-3".to_string());
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('g'))));
    assert!(cmds.is_empty(), "lone g must not fire immediately");
    assert!(
        matches!(
            app.interaction.pending,
            crate::tui::PendingAction::GChord(_)
        ),
        "g must start a pending gg-chord window"
    );
}

#[test]
fn handle_key_normal_g_then_other_key_abandons_chord_and_processes_key() {
    let mut app = make_app();
    // Backlog (nav col 1) has two tasks, so `j` has a visible effect.
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    without_usage(app.handle_key(make_key(KeyCode::Char('g'))));
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('j'))));
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { .. })
        )),
        "a lone g with no chord follow-up fires no jump action"
    );
    assert_eq!(
        app.selection().row(1),
        1,
        "the abandoned chord's key (j) must still be processed normally"
    );
    assert!(
        matches!(app.interaction.pending, crate::tui::PendingAction::None),
        "chord must be cleared once abandoned"
    );
}

#[test]
fn handle_key_normal_gg_jumps_to_top_without_firing_jump_window() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
        make_task(3, TaskStatus::Backlog),
    ]);
    app.selection_mut().set_column(1);
    app.update(Message::NavigateRow(1));
    app.update(Message::NavigateRow(1));
    assert_eq!(app.selection().row(1), 2, "precondition: not at top");

    without_usage(app.handle_key(make_key(KeyCode::Char('g'))));
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('g'))));

    assert_eq!(app.selection().row(1), 0, "gg should jump to top of column");
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { .. })
        )),
        "gg must not also fire the jump-to-window action"
    );
    assert!(matches!(
        app.interaction.pending,
        crate::tui::PendingAction::None
    ));
}

#[test]
fn handle_key_normal_g_idle_backstop_clears_pending_chord() {
    let mut app = make_app();
    let task = app
        .board
        .tasks
        .iter_mut()
        .find(|t| t.id == TaskId(3))
        .unwrap();
    task.tmux_window = Some("main:task-3".to_string());
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);

    without_usage(app.handle_key(make_key(KeyCode::Char('g'))));
    assert!(matches!(
        app.interaction.pending,
        crate::tui::PendingAction::GChord(_)
    ));
    let cmds = resolve_pending_g_via_idle_tick(&mut app);
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { .. })
        )),
        "an abandoned lone g fires no action, even via the idle backstop"
    );
    assert!(matches!(
        app.interaction.pending,
        crate::tui::PendingAction::None
    ));
}

#[test]
fn handle_key_normal_shift_g_jumps_to_last_row() {
    let mut app = make_app();
    app.selection_mut().set_column(1); // Backlog: tasks 1, 2
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('G'))));
    assert!(cmds.is_empty());
    assert_eq!(app.selection().row(1), 1, "G should jump to last task");
}

#[test]
fn handle_key_normal_shift_g_on_empty_column_is_noop() {
    let mut app = App::new(vec![]);
    app.selection_mut().set_column(1); // empty Backlog
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('G'))));
    assert!(cmds.is_empty());
    assert_eq!(app.selection().row(1), 0, "row unchanged on empty column");
}

#[test]
fn handle_key_normal_open_pr_url() {
    let mut app = make_app();
    let task = app
        .board
        .tasks
        .iter_mut()
        .find(|t| t.id == TaskId(1))
        .unwrap();
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/example/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('p')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::System(crate::tui::commands::SystemCommand::OpenInBrowser { url }) if url == "https://github.com/example/repo/pull/42"
    )));
}

#[test]
fn handle_key_normal_open_pr_url_missing() {
    let mut app = make_app();
    // task 1 has no pr_url by default
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);

    let cmds = app.handle_key(make_key(KeyCode::Char('p')));
    assert!(cmds.is_empty());
    assert!(app.status.message.as_deref().unwrap().contains("No URL"));
}

#[test]
fn esc_clears_mixed_selection() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    app.update(Message::Epic(
        crate::tui::messages::EpicMessage::ToggleSelect(EpicId(10)),
    ));

    app.handle_key(make_key(KeyCode::Esc));
    assert!(app.select.tasks.is_empty());
    assert!(app.select.epics.is_empty());
}

#[test]
fn confirm_detach_tmux_clears_window() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)]);
    app.board.tasks[0].tmux_window = Some("task-1".to_string());
    app.board.tasks[0].sub_status = SubStatus::Stale;
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::DetachTmux(TaskId(1)),
    ));
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::ConfirmDetachTmux,
    ));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(
        app.board.tasks[0].tmux_window.is_none(),
        "tmux_window should be cleared"
    );
    assert_ne!(
        app.find_task(TaskId(1)).unwrap().sub_status,
        SubStatus::Stale,
        "stale tracking should be cleared"
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::KillTmuxWindow { window }) if window == "task-1")),
        "should emit KillTmuxWindow for task-1"
    );
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Persist(_))
        )),
        "should emit PersistTask"
    );
}

#[test]
fn confirm_detach_tmux_emits_a_draining_subagent_clear() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Review)]);
    app.board.tasks[0].tmux_window = Some("task-1".to_string());
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::DetachTmux(TaskId(1)),
    ));
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::ConfirmDetachTmux,
    ));

    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::ClearSubagents {
                id,
                mode: crate::models::DrainMode::Drain,
            }) if *id == TaskId(1)
        )),
        "detach must clear subagents via the drain path — a genuinely finished agent's pending Stop should land"
    );
}

#[test]
fn confirm_detach_tmux_y_detaches() {
    let mut task = make_task(3, TaskStatus::Review);
    task.tmux_window = Some("task-3".to_string());
    let mut app = App::new(vec![task]);
    app.input.mode = InputMode::ConfirmDetachTmux(vec![TaskId(3)]);
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    // Should produce KillTmuxWindow + PatchSubStatus commands
    assert!(!cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_detach_tmux_n_cancels() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDetachTmux(vec![TaskId(3)]);
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn handle_key_normal_copy_task() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    app.handle_key(make_key(KeyCode::Char('c')));
    // CopyTask skips title/tag and goes straight to repo path with pre-filled buffer
    assert_eq!(*app.mode(), InputMode::InputRepoPath);
    assert!(app
        .input
        .task_draft
        .as_ref()
        .unwrap()
        .title
        .contains("Task 1"));
}

#[test]
fn handle_key_normal_toggle_notifications() {
    let mut app = make_app();
    let before = app.notifications_enabled;
    app.handle_key(make_key(KeyCode::Char('N')));
    assert_ne!(app.notifications_enabled, before);
}

#[test]
fn handle_key_normal_move_forward_via_handle_key() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('L')));
    // Task 1 should move from Backlog to Running
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Persist(t)) if t.id == TaskId(1) && t.status == TaskStatus::Running)));
}

#[test]
fn handle_key_normal_move_backward_via_handle_key() {
    let mut app = make_app();
    // Select running task (column 2)
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('H')));
    // Task 3 should move from Running to Backlog
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Persist(t)) if t.id == TaskId(3) && t.status == TaskStatus::Backlog)));
}

#[test]
fn handle_key_normal_detach_tmux_review_task() {
    let mut task = make_task(10, TaskStatus::Review);
    task.tmux_window = Some("main:10-test".to_string());
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(3);
    app.selection_mut().set_row(3, 0);
    app.handle_key(make_key(KeyCode::Char('T')));
    assert!(matches!(*app.mode(), InputMode::ConfirmDetachTmux(_)));
}

#[test]
fn handle_key_normal_detach_tmux_no_window_is_noop() {
    let mut app = make_app();
    // Task 1 has no tmux window
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('T')));
    assert!(cmds.is_empty());
}

#[test]
fn handle_key_normal_detach_tmux_running_task_with_window_prompts() {
    // Running tasks with a tmux window should also be detachable via T.
    let mut task = make_task(20, TaskStatus::Running);
    task.tmux_window = Some("main:20-running".to_string());
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(2); // Running column
    app.selection_mut().set_row(2, 0);
    app.handle_key(make_key(KeyCode::Char('T')));
    assert!(
        matches!(app.mode(), InputMode::ConfirmDetachTmux(ids) if ids == &[TaskId(20)]),
        "Expected ConfirmDetachTmux([20]), got {:?}",
        app.mode()
    );
}

#[test]
fn handle_key_normal_unknown_key_is_noop() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('z')));
    assert!(cmds.is_empty());
}

#[test]
fn handle_key_text_input_repo_j_types_into_buffer() {
    // j should be a typeable character in the repo path search box
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.buffer.clear();
    app.input.repo_cursor = 0;

    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.input.buffer, "j");
    assert_eq!(app.input.repo_cursor, 0); // cursor resets on query change
}

#[test]
fn handle_key_text_input_repo_k_types_into_buffer() {
    // k should be a typeable character in the repo path search box
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.buffer.clear();
    app.input.repo_cursor = 0;

    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.input.buffer, "k");
    assert_eq!(app.input.repo_cursor, 0); // cursor resets on query change
}

#[test]
fn handle_key_text_input_repo_jk_typed_together() {
    // Typing "jk" should appear in the search buffer
    let mut app = make_app();
    app.board.repo_paths = vec!["/jk-repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.buffer.clear();

    app.handle_key(make_key(KeyCode::Char('j')));
    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.input.buffer, "jk");
}

#[test]
fn handle_key_text_input_repo_arrow_down_navigates() {
    // Arrow keys should still navigate the list
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.buffer.clear();
    app.input.repo_cursor = 0;

    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 1);
}

#[test]
fn handle_key_text_input_repo_arrow_up_navigates() {
    // Arrow keys should still navigate the list
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo".to_string(), "/other".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.buffer.clear();
    app.input.repo_cursor = 1;

    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn handle_key_text_input_repo_enter_selects_cursor_repo() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/tmp".to_string(), "/var".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        description: "desc".to_string(),
        ..Default::default()
    });
    app.input.buffer.clear();
    app.input.repo_cursor = 1;

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    // Advances to InputBaseBranch; task not created until wrap-up mode selected
    assert_eq!(app.input.mode, InputMode::InputBaseBranch);
    assert!(cmds.is_empty());
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitBaseBranch("main".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputWrapUpMode);
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitWrapUpMode(None),
    ));
    // Declining the schedule gate is what commits the creation.
    let cmds3 = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitScheduleGate(false),
    ));
    assert!(cmds3.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Insert { .. })
    )));
}

#[test]
fn handle_key_text_input_enter_submits_typed_text() {
    let mut app = make_app();
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        description: "desc".to_string(),
        ..Default::default()
    });
    app.input.set_buffer("/tmp".to_string());

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    // Advances to InputBaseBranch; task not created until wrap-up mode selected
    assert_eq!(app.input.mode, InputMode::InputBaseBranch);
    assert!(cmds.is_empty());
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitBaseBranch("main".to_string()),
    ));
    assert_eq!(app.input.mode, InputMode::InputWrapUpMode);
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitWrapUpMode(None),
    ));
    // Declining the schedule gate is what commits the creation.
    let cmds3 = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitScheduleGate(false),
    ));
    assert!(cmds3.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Insert { .. })
    )));
}

// ── QuickDispatch picker: text-input contract (no j/k or digit hijacks) ──

fn quick_dispatch_app(paths: &[&str]) -> App {
    let mut app = make_app();
    app.board.repo_paths = paths.iter().map(|s| s.to_string()).collect();
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 0;
    app.input.buffer.clear();
    app
}

#[test]
fn handle_key_quick_dispatch_j_typed_into_buffer() {
    let mut app = quick_dispatch_app(&["/jkl/repo", "/abc/repo"]);
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.input.buffer, "j");
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn handle_key_quick_dispatch_k_typed_into_buffer() {
    let mut app = quick_dispatch_app(&["/kong/repo", "/abc/repo"]);
    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.input.buffer, "k");
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn handle_key_quick_dispatch_digits_typed_into_buffer() {
    for c in '0'..='9' {
        let mut app = quick_dispatch_app(&["/repo-1", "/repo-2", "/repo-3"]);
        let cmds = app.handle_key(make_key(KeyCode::Char(c)));
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { .. })
            )),
            "digit '{c}' must not select"
        );
        assert_eq!(app.input.buffer, c.to_string(), "digit '{c}'");
        assert_eq!(app.input.repo_cursor, 0, "digit '{c}'");
    }
}

#[test]
fn handle_key_quick_dispatch_down_arrow_navigates() {
    let mut app = quick_dispatch_app(&["/a", "/b"]);
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 1);
    assert!(app.input.buffer.is_empty());
}

#[test]
fn handle_key_quick_dispatch_up_arrow_navigates() {
    let mut app = quick_dispatch_app(&["/a", "/b"]);
    app.input.repo_cursor = 1;
    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn handle_key_quick_dispatch_enter_selects_cursor_entry() {
    let mut app = quick_dispatch_app(&["/a", "/b"]);
    app.input.repo_cursor = 1;
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { .. })
    )));
}

#[test]
fn handle_key_quick_dispatch_backspace_pops_and_resets_cursor() {
    let mut app = quick_dispatch_app(&["/repo"]);
    app.input.set_buffer("abc".to_string());
    app.input.repo_cursor = 2;
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "ab");
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn handle_key_quick_dispatch_esc_cancels_and_clears_buffer() {
    let mut app = quick_dispatch_app(&["/repo"]);
    app.input.set_buffer("abc".to_string());
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
}

#[test]
fn handle_key_quick_dispatch_typing_digit_filters_by_digit() {
    // Regression: with paths containing digits, typing a digit must
    // filter (subsequence) rather than instant-select by index.
    let mut app = quick_dispatch_app(&["/foo-1", "/bar-2"]);
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { .. })
        )),
        "typing '2' must not select"
    );
    assert_eq!(app.input.buffer, "2");
    let filtered = crate::tui::filtered_repos(&app.board.repo_paths, &app.input.buffer);
    assert_eq!(filtered, vec!["/bar-2".to_string()]);
}

#[test]
fn handle_key_quick_dispatch_typing_j_filters_by_j() {
    // Regression: typing 'j' must filter, not navigate.
    let mut app = quick_dispatch_app(&["/jkl/repo", "/abc/repo"]);
    app.handle_key(make_key(KeyCode::Char('j')));
    let filtered = crate::tui::filtered_repos(&app.board.repo_paths, &app.input.buffer);
    assert_eq!(filtered, vec!["/jkl/repo".to_string()]);
    assert_eq!(app.input.repo_cursor, 0);
}

// ── InputRepoPath / MainSessionDir digit-filtering regressions ──

#[test]
fn handle_key_input_repo_path_typing_digit_filters_not_selects() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-1".to_string(), "/repo-2".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Insert { .. })
        )),
        "digit must not submit a repo path; cmds: {cmds:?}"
    );
    assert_eq!(app.input.buffer, "2");
}

#[test]
fn handle_key_main_session_dir_typing_digit_filters_not_selects() {
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo-1".to_string(), "/repo-2".to_string()];
    app.input.mode = InputMode::MainSessionDir;
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert!(
        !cmds.iter().any(
            |c| matches!(c, Command::Settings(SettingsCommand::PersistStringSetting { key, .. }) if key == "main_session.dir")
        ),
        "digit must not submit a main session dir; cmds: {cmds:?}"
    );
    assert_eq!(app.input.buffer, "2");
}

#[test]
fn handle_key_tag_selects_feature() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    app.handle_key(make_key(KeyCode::Char('f')));
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().tag,
        Some(TaskTag::Feature)
    );
}

#[test]
fn handle_key_tag_selects_chore() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    app.handle_key(make_key(KeyCode::Char('c')));
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().tag,
        Some(TaskTag::Chore)
    );
}

#[test]
fn handle_key_tag_unknown_key_is_noop() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    let cmds = app.handle_key(make_key(KeyCode::Char('z')));
    assert!(cmds.is_empty());
    assert_eq!(*app.mode(), InputMode::InputTag);
}

#[test]
fn handle_key_confirm_detach_tmux_non_matching_mode_is_noop() {
    let mut app = make_app();
    // Mode is Normal but we call handle_key_confirm_detach_tmux indirectly
    // This shouldn't happen in practice, but confirms guard clause
    app.input.mode = InputMode::Normal;
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    // In Normal mode, 'y' is unrecognized — noop
    assert!(cmds.is_empty());
}

/// In Normal mode on the Board view, known keys produce commands/state changes
/// and unknown keys produce no commands.
#[test]
fn handle_key_normal_board_known_keys_produce_effects() {
    let mut app = make_app();
    // 'n' starts new task (switches to InputTitle mode)
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(cmds.is_empty()); // inline mutation, no commands
    assert_eq!(app.input.mode, InputMode::InputTitle);
}

#[test]
fn handle_key_normal_board_unknown_key_is_noop() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('z')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// `I` used to open the knowledge-base overlay, removed in
/// docs/plans/archive/2026-07-31-3809-keybinding-pruning-implementation.md §3. It is now unbound:
/// no match arm, so it must fall through exactly like any unknown key. Learnings
/// are curated via MCP only.
#[test]
fn handle_key_normal_board_learnings_key_is_unbound() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('I')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

/// `C` used to open the managed-feed config popup, removed in
/// docs/plans/archive/2026-07-31-3809-keybinding-pruning-implementation.md §6. It is now unbound:
/// no match arm, so it must fall through exactly like any unknown key. The four
/// managed-feed settings are configured via MCP only
/// (`set_managed_feed_config` / `get_managed_feed_config`).
#[test]
fn handle_key_normal_board_feed_config_key_is_unbound() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('C')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(matches!(app.board.view_mode, ViewMode::Board(_)));
}

/// InputTitle mode routes to the text input handler.
#[test]
fn handle_key_input_title_routes_to_text_input() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTitle;
    // Esc cancels input
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// InputDescription mode routes to the text input handler.
#[test]
fn handle_key_input_description_routes_to_text_input() {
    let mut app = make_app();
    app.input.mode = InputMode::InputDescription;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// InputRepoPath mode routes to the text input handler.
#[test]
fn handle_key_input_repo_path_routes_to_text_input() {
    let mut app = make_app();
    app.input.mode = InputMode::InputRepoPath;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// ConfirmDelete mode routes to the confirm-delete handler.
#[test]
fn handle_key_confirm_delete_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDelete;
    // 'n' cancels the delete
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// InputTag mode routes to the tag handler.
#[test]
fn handle_key_input_tag_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    // Esc cancels tag input
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// QuickDispatch mode routes to the quick-dispatch handler.
#[test]
fn handle_key_quick_dispatch_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::QuickDispatch;
    // Esc cancels quick dispatch
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// ConfirmRetry mode routes to the confirm-retry handler.
#[test]
fn handle_key_confirm_retry_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmRetry(TaskId(1));
    // Esc cancels retry
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// ConfirmDetachTmux mode routes correctly.
#[test]
fn handle_key_confirm_detach_tmux_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmDetachTmux(vec![TaskId(1)]);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// Help mode routes to the help handler.
#[test]
fn handle_key_help_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;
    // Any key exits help
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// ConfirmQuit mode routes correctly.
#[test]
fn handle_key_confirm_quit_routes_correctly() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmQuit;
    // 'n' cancels quit
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

/// Error popup dismisses on any key before routing to normal handler.
#[test]
fn handle_key_error_popup_dismisses_first() {
    let mut app = make_app();
    app.status.error_popup = Some("Some error".to_string());
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('x'))));
    assert!(cmds.is_empty());
    assert!(app.status.error_popup.is_none());
}

#[test]
fn confirm_quit_without_split_emits_no_extra_commands() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmQuit;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('y'))));

    assert!(app.should_quit);
    assert!(cmds.is_empty(), "no commands when split is not active");
}

#[test]
fn enter_with_typed_filter_selects_filtered_item() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp".to_string(), "/var".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    // Type "var" — only /var matches, cursor = 0
    type_text(&mut app, "var");
    // Enter selects /var
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputBaseBranch);
    assert_eq!(app.input.task_draft.as_ref().unwrap().repo_path, "/var");
}

#[test]
fn handle_key_tag_selects_pr_review() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    app.handle_key(make_key(KeyCode::Char('p')));
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().tag,
        Some(TaskTag::PrReview)
    );
}

#[test]
fn handle_key_tag_selects_research() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    app.handle_key(make_key(KeyCode::Char('r')));
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().tag,
        Some(TaskTag::Research)
    );
}

#[test]
fn handle_key_tag_selects_fix() {
    let mut app = make_app();
    app.input.mode = InputMode::InputTag;
    app.input.task_draft = Some(TaskDraft {
        title: "Test".to_string(),
        ..Default::default()
    });

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(
        app.input.task_draft.as_ref().unwrap().tag,
        Some(TaskTag::Fix)
    );
}

// ---------------------------------------------------------------------------
// InputWrapUpMode tests
// ---------------------------------------------------------------------------

#[test]
fn submit_base_branch_transitions_to_wrap_up_mode() {
    let mut app = make_app();
    app.input.mode = InputMode::InputBaseBranch;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        repo_path: "/repo".to_string(),
        base_branch: "main".into(),
        ..Default::default()
    });
    app.input.set_buffer("main".to_string());

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));

    assert_eq!(
        app.input.mode,
        InputMode::InputWrapUpMode,
        "expected InputWrapUpMode after submitting base branch, got {:?}",
        app.input.mode
    );
    assert!(
        cmds.is_empty(),
        "no commands should be emitted before wrap-up mode selection"
    );
}

/// A draft parked at the wrap-up step — the shared starting point for every
/// test of the form's tail (wrap-up picker, schedule gate, and the two
/// configure steps beyond it).
fn app_at_wrap_up_step() -> App {
    let mut app = make_app();
    app.input.mode = InputMode::InputWrapUpMode;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        repo_path: "/repo".to_string(),
        base_branch: "main".into(),
        ..Default::default()
    });
    app
}

/// The draft the form asked to create, or `None` if it emitted no Insert.
fn insert_draft(cmds: &[Command]) -> Option<&TaskDraft> {
    cmds.iter().find_map(|c| match c {
        Command::Task(crate::tui::commands::TaskCommand::Insert { draft, .. }) => Some(draft),
        _ => None,
    })
}

/// Type a string into the active text field, one key at a time.
fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(make_key(KeyCode::Char(c)));
    }
}

#[test]
fn wrap_up_mode_r_selects_rebase_and_creates_task() {
    let mut app = app_at_wrap_up_step();

    app.handle_key(make_key(KeyCode::Char('r')));
    // The schedule gate is the form's last step; declining it is what commits
    // the creation (tasks.allium: CreateTask, "The schedule step").
    let cmds = app.handle_key(make_key(KeyCode::Enter));

    assert_eq!(app.input.mode, InputMode::Normal);
    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(
        draft.wrap_up_mode,
        Some(crate::models::WrapUpMode::Rebase),
        "expected Rebase wrap_up_mode"
    );
}

#[test]
fn wrap_up_mode_p_selects_pr_and_creates_task() {
    let mut app = app_at_wrap_up_step();

    app.handle_key(make_key(KeyCode::Char('p')));
    // The schedule gate is the form's last step; declining it is what commits
    // the creation (tasks.allium: CreateTask, "The schedule step").
    let cmds = app.handle_key(make_key(KeyCode::Enter));

    assert_eq!(app.input.mode, InputMode::Normal);
    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(
        draft.wrap_up_mode,
        Some(crate::models::WrapUpMode::Pr),
        "expected Pr wrap_up_mode"
    );
}

#[test]
fn wrap_up_mode_d_selects_done_and_creates_task() {
    let mut app = app_at_wrap_up_step();

    app.handle_key(make_key(KeyCode::Char('d')));
    // The schedule gate is the form's last step; declining it is what commits
    // the creation (tasks.allium: CreateTask, "The schedule step").
    let cmds = app.handle_key(make_key(KeyCode::Enter));

    assert_eq!(app.input.mode, InputMode::Normal);
    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(
        draft.wrap_up_mode,
        Some(crate::models::WrapUpMode::Done),
        "expected Done wrap_up_mode"
    );
}

#[test]
fn wrap_up_mode_enter_skips_and_creates_task_with_no_mode() {
    let mut app = app_at_wrap_up_step();

    app.handle_key(make_key(KeyCode::Enter));
    // Enter again: the schedule gate's default is "not scheduled".
    let cmds = app.handle_key(make_key(KeyCode::Enter));

    assert_eq!(app.input.mode, InputMode::Normal);
    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(
        draft.wrap_up_mode, None,
        "Enter should create task with no wrap-up mode"
    );
}

#[test]
fn wrap_up_mode_enter_keeps_prefilled_value_from_copy_task() {
    // Regression guard: CopyTask prefills wrap_up_mode from the source task,
    // but the picker's Enter previously always submitted None, silently
    // clearing it. Enter (no explicit r/p/d pick) must keep whatever the
    // draft already carries.
    let mut app = app_at_wrap_up_step();
    if let Some(draft) = app.input.task_draft.as_mut() {
        draft.title = "Copy of: T".to_string();
        draft.wrap_up_mode = Some(crate::models::WrapUpMode::Pr);
    }

    app.handle_key(make_key(KeyCode::Enter));
    // Enter again: the schedule gate's default is "not scheduled".
    let cmds = app.handle_key(make_key(KeyCode::Enter));

    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(
        draft.wrap_up_mode,
        Some(crate::models::WrapUpMode::Pr),
        "Enter should keep the copied wrap_up_mode, not clear it"
    );
}

// ---------------------------------------------------------------------------
// Schedule gate + schedule-interval / pinned-branch steps
// ---------------------------------------------------------------------------

#[test]
fn wrap_up_mode_now_falls_through_to_the_schedule_gate_not_straight_to_creation() {
    let mut app = app_at_wrap_up_step();

    let cmds = app.handle_key(make_key(KeyCode::Char('r')));

    assert_eq!(app.input.mode, InputMode::InputScheduleGate);
    assert!(
        insert_draft(&without_usage(cmds)).is_none(),
        "the gate must be answered before the task is created"
    );
}

/// The common path, and the whole reason the gate exists: one keypress and the
/// task is created, with neither scheduling field set.
#[test]
fn schedule_gate_enter_creates_the_task_unscheduled() {
    let mut app = app_at_wrap_up_step();
    app.handle_key(make_key(KeyCode::Char('r')));

    let cmds = app.handle_key(make_key(KeyCode::Enter));

    assert_eq!(app.input.mode, InputMode::Normal);
    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(draft.schedule_interval_secs, None);
    assert_eq!(draft.pinned_branch, None);
    assert_eq!(
        draft.wrap_up_mode,
        Some(crate::models::WrapUpMode::Rebase),
        "passing through the gate must not disturb the earlier steps"
    );
}

#[test]
fn schedule_gate_s_opens_the_interval_step_without_creating_the_task() {
    let mut app = app_at_wrap_up_step();
    app.handle_key(make_key(KeyCode::Char('r')));

    let cmds = app.handle_key(make_key(KeyCode::Char('s')));

    assert_eq!(app.input.mode, InputMode::InputScheduleInterval);
    assert!(insert_draft(&without_usage(cmds)).is_none());
}

/// Same contract as every other picker: an unrecognised key is ignored and the
/// prompt stays open, rather than falling through to the default.
#[test]
fn schedule_gate_ignores_an_unrecognised_key() {
    let mut app = app_at_wrap_up_step();
    app.handle_key(make_key(KeyCode::Char('r')));

    let cmds = app.handle_key(make_key(KeyCode::Char('z')));

    assert_eq!(app.input.mode, InputMode::InputScheduleGate);
    assert!(without_usage(cmds).is_empty());
}

#[test]
fn schedule_gate_esc_cancels_creation() {
    let mut app = app_at_wrap_up_step();
    app.handle_key(make_key(KeyCode::Char('r')));

    app.handle_key(make_key(KeyCode::Esc));

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
}

/// Drive the whole configure branch and check both fields land on the draft.
#[test]
fn configuring_both_fields_carries_them_onto_the_created_task() {
    let mut app = app_at_wrap_up_step();
    app.handle_key(make_key(KeyCode::Enter)); // wrap-up: skip
    app.handle_key(make_key(KeyCode::Char('s'))); // gate: configure

    type_text(&mut app, "600");
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputPinnedBranch);
    assert!(
        app.input.buffer.is_empty(),
        "the interval buffer must be cleared before the branch step reuses it"
    );

    type_text(&mut app, "staging");
    let cmds = app.handle_key(make_key(KeyCode::Enter));

    assert_eq!(app.input.mode, InputMode::Normal);
    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(draft.schedule_interval_secs, Some(600));
    assert_eq!(draft.pinned_branch, Some("staging".to_string()));
}

/// The shared interval grammar reaches the form, so a human types "10m".
#[test]
fn the_interval_step_accepts_a_suffixed_literal() {
    let mut app = app_at_wrap_up_step();
    app.handle_key(make_key(KeyCode::Enter));
    app.handle_key(make_key(KeyCode::Char('s')));

    type_text(&mut app, "10m");
    app.handle_key(make_key(KeyCode::Enter));
    let cmds = app.handle_key(make_key(KeyCode::Enter)); // skip pinned branch

    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(draft.schedule_interval_secs, Some(600));
}

/// Taking the "s" branch does not commit the user to setting both: Enter on an
/// empty buffer leaves that one field null and advances.
#[test]
fn each_configure_step_is_individually_skippable() {
    let mut app = app_at_wrap_up_step();
    app.handle_key(make_key(KeyCode::Enter));
    app.handle_key(make_key(KeyCode::Char('s')));

    app.handle_key(make_key(KeyCode::Enter)); // skip the interval
    assert_eq!(app.input.mode, InputMode::InputPinnedBranch);

    type_text(&mut app, "staging");
    let cmds = app.handle_key(make_key(KeyCode::Enter));

    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(draft.schedule_interval_secs, None);
    assert_eq!(draft.pinned_branch, Some("staging".to_string()));
}

/// The one form step that refuses to advance on non-empty input: the value
/// drives a background loop the user will not be watching, so garbage must not
/// be coerced or silently dropped.
#[test]
fn an_unparseable_interval_is_rejected_in_place() {
    for bad in ["abc", "0", "-5", "10w"] {
        let mut app = app_at_wrap_up_step();
        app.handle_key(make_key(KeyCode::Enter));
        app.handle_key(make_key(KeyCode::Char('s')));
        type_text(&mut app, bad);

        let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));

        assert_eq!(
            app.input.mode,
            InputMode::InputScheduleInterval,
            "{bad:?} should not have advanced the step"
        );
        assert!(
            insert_draft(&cmds).is_none(),
            "{bad:?} should not have created a task"
        );
        assert_eq!(
            app.input.buffer, bad,
            "{bad:?} should stay in the buffer so it can be corrected"
        );
        assert!(
            app.status.message.is_some(),
            "{bad:?} should surface an error"
        );
    }
}

/// A rejected entry must be correctable in place rather than needing the form
/// restarted.
#[test]
fn a_rejected_interval_can_be_corrected_and_resubmitted() {
    let mut app = app_at_wrap_up_step();
    app.handle_key(make_key(KeyCode::Enter));
    app.handle_key(make_key(KeyCode::Char('s')));
    type_text(&mut app, "abc");
    app.handle_key(make_key(KeyCode::Enter));

    for _ in 0..3 {
        app.handle_key(make_key(KeyCode::Backspace));
    }
    type_text(&mut app, "5m");
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputPinnedBranch);

    let cmds = app.handle_key(make_key(KeyCode::Enter));
    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(draft.schedule_interval_secs, Some(300));
}

/// Neither step is a picker: every printable character is literal text, so a
/// branch name containing a picker's navigation keys types through intact.
#[test]
fn the_branch_step_treats_every_printable_character_as_text() {
    let mut app = app_at_wrap_up_step();
    app.handle_key(make_key(KeyCode::Enter));
    app.handle_key(make_key(KeyCode::Char('s')));
    app.handle_key(make_key(KeyCode::Enter)); // skip the interval

    type_text(&mut app, "jk-release/v2");
    let cmds = app.handle_key(make_key(KeyCode::Enter));

    let draft = insert_draft(&cmds).expect("expected Insert command");
    assert_eq!(draft.pinned_branch, Some("jk-release/v2".to_string()));
}

#[test]
fn esc_from_either_configure_step_cancels_creation() {
    for mode in [
        InputMode::InputScheduleInterval,
        InputMode::InputPinnedBranch,
    ] {
        let mut app = app_at_wrap_up_step();
        app.input.mode = mode.clone();

        app.handle_key(make_key(KeyCode::Esc));

        assert_eq!(app.input.mode, InputMode::Normal, "from {mode:?}");
        assert!(app.input.task_draft.is_none(), "from {mode:?}");
    }
}

// ---------------------------------------------------------------------------
// Normal-mode handler coverage for extracted methods
// ---------------------------------------------------------------------------

#[test]
fn space_key_on_split_pinned_task_focuses_pane() {
    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task]);
    app.board.split.active = true;
    app.board.split.right_pane_id = Some("%42".to_string());
    app.board.split.pinned_task_id = Some(TaskId(4));
    app.selection_mut().set_column(2);

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Split(crate::tui::commands::SplitCommand::FocusPane { pane_id }) if pane_id == "%42"
        )),
        "pinned task should focus the split pane, got {cmds:?}"
    );
}

#[test]
fn space_key_on_epic_enters_epic_view() {
    let mut app = make_app_with_epic_selected();
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(
        matches!(app.board.view_mode, ViewMode::Epic { epic_id, .. } if epic_id == EpicId(10)),
        "space on epic should enter ViewMode::Epic, got {:?}",
        app.board.view_mode
    );
}

#[test]
fn space_on_task_in_active_split_swaps_pane() {
    let mut task = make_task(3, TaskStatus::Running);
    task.tmux_window = Some("task-3".to_string());
    let mut app = App::new(vec![task]);
    app.board.split.active = true;
    app.board.split.right_pane_id = Some("%10".to_string());
    app.selection_mut().set_column(2);

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Split(crate::tui::commands::SplitCommand::Swap { .. })
        )),
        "Space on task with split active should swap pane, got {cmds:?}"
    );
}

#[test]
fn capital_s_is_inert() {
    // [S] retired in favour of Space-in-split-mode: no arm, no hint.
    let mut task = make_task(1, TaskStatus::Backlog);
    task.tmux_window = Some("task-1".to_string());
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(1);

    let cmds = app.handle_key(make_key(KeyCode::Char('S')));
    assert!(cmds.is_empty(), "S must emit no commands, got {cmds:?}");
    assert!(
        app.status.message.is_none(),
        "S must show no hint, got {:?}",
        app.status.message
    );
}

#[test]
fn capital_g_on_epic_is_noop() {
    let anchor_task = make_task(1, TaskStatus::Backlog);
    let mut running_blocked = make_task(2, TaskStatus::Running);
    running_blocked.epic_id = Some(EpicId(10));
    running_blocked.sub_status = SubStatus::Stale;
    running_blocked.tmux_window = Some("task-blocked".to_string());

    let mut app = App::new(vec![anchor_task, running_blocked]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1);

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('G'))));
    assert!(cmds.is_empty(), "G on an epic must emit no commands");
}

#[test]
fn r_key_on_feed_epic_triggers_feed() {
    // task id=1 at row 0, epic id=10 at row 1 in Backlog column
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let mut epic = make_epic(10);
    epic.feed_command = Some("gh api ...".to_string());
    app.board.epics = vec![epic];
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1); // cursor on epic at row 1

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('r'))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Feed(crate::tui::commands::FeedCommand::TriggerEpic { epic_id, .. }) if *epic_id == EpicId(10)
        )),
        "r on feed epic should trigger feed, got {cmds:?}"
    );
}

#[test]
fn r_key_inside_epic_view_with_feed_triggers_feed() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let mut epic = make_epic(10);
    epic.feed_command = Some("gh api ...".to_string());
    app.board.epics = vec![epic];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        EpicId(10),
    )));

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('r'))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Feed(crate::tui::commands::FeedCommand::TriggerEpic { epic_id, .. }) if *epic_id == EpicId(10)
        )),
        "r inside epic view with feed should trigger feed, got {cmds:?}"
    );
}

#[test]
fn r_key_without_feed_epic_is_noop() {
    let mut app = make_app(); // tasks, no feed epics
    app.selection_mut().set_column(1);

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('r'))));
    assert!(cmds.is_empty(), "r without a feed epic should be noop");
}

#[test]
fn capital_d_with_no_repos_opens_picker() {
    // With no saved repos, D should open the QuickDispatch picker so the user
    // can type a new repo path — the old "No saved repo paths" error is gone.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec![];
    app.selection_mut().set_column(1);

    without_usage(app.handle_key(make_key(KeyCode::Char('D'))));
    assert!(
        matches!(app.input.mode, InputMode::QuickDispatch),
        "D with no repos should open the picker, got {:?}",
        app.input.mode
    );
}

#[test]
fn capital_d_with_one_repo_quick_dispatches() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/repo".to_string()];
    app.selection_mut().set_column(1);

    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('D'))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { .. })
        )),
        "D with 1 repo should emit QuickDispatch, got {cmds:?}"
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn capital_d_with_multiple_repos_opens_selection() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    app.selection_mut().set_column(1);

    without_usage(app.handle_key(make_key(KeyCode::Char('D'))));
    assert_eq!(
        app.input.mode,
        InputMode::QuickDispatch,
        "D with multiple repos should open selection UI"
    );
}

#[test]
fn enter_key_when_on_select_all_deselects_column() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    // Navigate up from row 0 to reach the select-all toggle
    app.update(Message::NavigateRow(-1));
    assert!(app.on_select_all(), "precondition: on_select_all");

    // First, select all
    app.update(Message::SelectAllColumn);
    assert!(!app.select.tasks.is_empty(), "precondition: tasks selected");

    // Enter should deselect
    app.handle_key(make_key(KeyCode::Enter));
    assert!(
        app.select.tasks.is_empty(),
        "Enter on select-all should deselect all"
    );
}

// ── repo-path bug: new path must be selectable even when existing paths fuzzy-match ──

#[test]
fn repo_path_cursor_count_includes_new_path_slot() {
    // When buffer is non-empty and doesn't exactly match an existing path,
    // the cursor range should include a slot for the new path entry.
    // Existing path "/tmp/other" fuzzy-matches "/tmp", so filtered is non-empty,
    // but the new-path slot must still exist.
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp/other".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: String::new(),
        ..Default::default()
    });
    app.input.set_buffer("/tmp".to_string());
    app.input.repo_cursor = 0;

    // Down arrow should move to cursor 1 (the new-path slot) because
    // has_new_repo_option("/tmp", ["/tmp/other"]) is true.
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 1);

    // Down again wraps back to 0 (only 2 effective entries).
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn repo_path_enter_at_new_path_slot_submits_typed_value() {
    // With buffer "/tmp" that fuzzy-matches existing "/tmp/other", navigating
    // to the new-path slot (cursor 1) and pressing Enter should submit "/tmp",
    // not "/tmp/other".
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp/other".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        description: String::new(),
        ..Default::default()
    });
    app.input.set_buffer("/tmp".to_string());
    app.input.repo_cursor = 1; // new-path slot

    let _cmds = app.handle_key(make_key(KeyCode::Enter));
    // Should have advanced to InputBaseBranch with "/tmp" as the repo path.
    assert_eq!(app.input.mode, InputMode::InputBaseBranch);
    assert_eq!(app.input.task_draft.as_ref().unwrap().repo_path, "/tmp");
}

#[test]
fn quick_dispatch_zero_repos_opens_picker() {
    // With no saved repos, pressing D should open the picker (QuickDispatch mode)
    // so the user can type a new path — not show a "no saved paths" error.
    let mut app = App::new(vec![]);
    // no repo_paths
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('D'))));

    assert!(
        matches!(app.input.mode, InputMode::QuickDispatch),
        "expected QuickDispatch mode after D with no repos, got {:?}",
        app.input.mode
    );
    // No QuickDispatch command yet — just the picker opened.
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { .. })
        )),
        "should not emit QuickDispatch command immediately"
    );
}

#[test]
fn quick_dispatch_zero_repos_new_path_entry_accepted() {
    // With no saved repos, the user opens the picker, types "/tmp", and presses
    // Enter — this should emit a QuickDispatch command for the new path.
    let mut app = App::new(vec![]);
    app.handle_key(make_key(KeyCode::Char('D'))); // open picker
    assert!(matches!(app.input.mode, InputMode::QuickDispatch));

    // Type "/tmp" character by character.
    type_text(&mut app, "/tmp");
    assert_eq!(app.input.buffer, "/tmp");

    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { draft, .. })
                if draft.repo_path == "/tmp"
        )),
        "expected QuickDispatch command with /tmp, got {:?}",
        cmds
    );
}

#[test]
fn slash_enters_search_mode_and_snapshots_query() {
    let mut app = App::new(vec![]);
    app.search.query = "old".to_string();
    app.handle_key(make_key(KeyCode::Char('/')));
    assert_eq!(app.input.mode, InputMode::SearchTasks);
    assert_eq!(app.search.saved, Some("old".to_string()));
}

#[test]
fn typing_in_search_updates_query_live() {
    let mut app = App::new(vec![]);
    app.handle_key(make_key(KeyCode::Char('/')));
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('b')));
    assert_eq!(app.search.query, "ab");
    assert_eq!(app.input.mode, InputMode::SearchTasks);
}

#[test]
fn backspace_in_search_removes_last_char() {
    let mut app = App::new(vec![]);
    app.handle_key(make_key(KeyCode::Char('/')));
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Char('b')));
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.search.query, "a");
}

#[test]
fn enter_commits_search_and_keeps_query() {
    let mut app = App::new(vec![]);
    app.handle_key(make_key(KeyCode::Char('/')));
    app.handle_key(make_key(KeyCode::Char('a')));
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert_eq!(app.search.query, "a");
    assert_eq!(app.search.saved, None);
}

#[test]
fn esc_in_search_restores_snapshot() {
    let mut app = App::new(vec![]);
    app.search.query = "old".to_string();
    app.handle_key(make_key(KeyCode::Char('/')));
    app.handle_key(make_key(KeyCode::Char('x')));
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert_eq!(app.search.query, "old");
    assert_eq!(app.search.saved, None);
}

#[test]
fn esc_in_normal_clears_active_search() {
    let mut app = App::new(vec![]);
    app.search.query = "active".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.search.query, "");
}
