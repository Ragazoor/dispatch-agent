#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{
    DispatchMode, EpicId, SubStatus, TaskId, TaskStatus, TaskTag, ACTIVE_THRESHOLD,
    DEFAULT_QUICK_TASK_TITLE,
};
use crossterm::event::KeyCode;
use std::time::{Duration, Instant};

#[test]
fn dispatch_only_backlog_tasks() {
    let mut app = make_app();

    // Task 1 is Backlog — should dispatch
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));
    assert!(matches!(
        cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    ));

    // Task 3 is Running — should not dispatch
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(3),
        DispatchMode::Dispatch,
    )));
    assert!(cmds.is_empty());

    // Task 4 is Done — should not dispatch
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(4),
        DispatchMode::Dispatch,
    )));
    assert!(cmds.is_empty());
}

#[test]
fn tick_checks_window_for_review_task_with_live_window() {
    let mut task = make_task(5, TaskStatus::Review);
    task.tmux_window = Some("task-5".to_string());
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));

    assert!(cmds.iter().any(|c| {
        if let Command::Task(crate::tui::commands::TaskCommand::BatchCheckWindows { windows }) = c {
            windows.iter().any(|(id, _)| *id == TaskId(5))
        } else {
            false
        }
    }));
}

#[test]
fn dispatch_from_running_is_noop() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task]);
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(4),
        DispatchMode::Dispatch,
    )));
    assert!(cmds.is_empty());
}

#[test]
fn dispatch_from_review_is_noop() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = Some("task-5".to_string());
    let mut app = App::new(vec![task]);
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(5),
        DispatchMode::Dispatch,
    )));
    assert!(cmds.is_empty());
}

#[test]
fn shift_d_with_one_repo_emits_quick_dispatch() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/repo".to_string()];
    let cmds = without_usage(app.handle_key(make_shift_key(KeyCode::Char('D'))));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, epic_id: None }) if draft.repo_path == "/repo")
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn shift_d_with_no_repos_opens_picker() {
    // With no saved repos, D should open the picker so the user can type a new
    // repo path rather than showing a "no saved paths" error.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec![];
    let cmds = without_usage(app.handle_key(make_shift_key(KeyCode::Char('D'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn shift_d_with_multiple_repos_enters_quick_dispatch_mode() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = without_usage(app.handle_key(make_shift_key(KeyCode::Char('D'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_mode_typed_digit_filters_not_selects() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert!(cmds.is_empty(), "digit must not produce any command");
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
    assert_eq!(app.input.buffer, "2");
}

#[test]
fn quick_dispatch_mode_invalid_number_appends_to_buffer() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/repo1".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('3')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
    assert_eq!(app.input.buffer, "3");
}

#[test]
fn quick_dispatch_mode_esc_cancels() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Esc)));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn quick_dispatch_message_emits_command() {
    let mut app = App::new(vec![]);
    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::QuickDispatch {
            repo_path: "/my/repo".to_string(),
            epic_id: None,
        },
    ));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, epic_id: None })
        if draft.title == DEFAULT_QUICK_TASK_TITLE && draft.repo_path == "/my/repo")
    );
}

#[test]
fn shift_d_in_epic_view_quick_dispatches_subtask_single_repo() {
    let mut app = App::new(vec![]);
    let epic = make_epic(10);
    app.board.epics = vec![epic];
    app.board.repo_paths = vec!["/my/repo".to_string()];
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    let cmds = without_usage(app.handle_key(make_shift_key(KeyCode::Char('D'))));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, epic_id: Some(EpicId(10)) })
        if draft.repo_path == "/my/repo"
    ));
}

#[test]
fn shift_d_in_epic_view_shows_repo_selection_with_multiple_repos() {
    let mut app = App::new(vec![]);
    let epic = make_epic(10);
    app.board.epics = vec![epic];
    app.board.repo_paths = vec!["/repo/a".to_string(), "/repo/b".to_string()];
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    let cmds = without_usage(app.handle_key(make_shift_key(KeyCode::Char('D'))));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
    assert_eq!(app.input.pending_epic_id, Some(EpicId(10)));
}

#[test]
fn shift_d_in_epic_view_repo_selection_dispatches_with_epic_id() {
    let mut app = App::new(vec![]);
    let epic = make_epic(10);
    app.board.epics = vec![epic];
    app.board.repo_paths = vec!["/repo/a".to_string(), "/repo/b".to_string()];
    app.board.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new_for_epic(),
        parent: Box::new(ViewMode::Board(BoardSelection::new())),
    };
    // Enter selection mode
    app.handle_key(make_shift_key(KeyCode::Char('D')));
    // Move cursor to second repo, then Enter to select.
    app.handle_key(make_key(KeyCode::Down));
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, epic_id: Some(EpicId(10)) })
        if draft.repo_path == "/repo/b"
    ));
}

#[test]
fn stale_agent_detected_when_last_pre_tool_use_old() {
    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("task-4".to_string());
    // Pre-tool-use older than ACTIVE_THRESHOLD → classifier returns Stale.
    task.last_pre_tool_use_at = Some(chrono::Utc::now() - chrono::Duration::minutes(10));
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert!(app.is_stale(TaskId(4)));
    assert!(cmds.iter().any(|c| {
        if let Command::Task(crate::tui::commands::TaskCommand::BatchCheckWindows { windows }) = c {
            windows.iter().any(|(id, _)| *id == TaskId(4))
        } else {
            false
        }
    }));
    assert!(cmds.iter().any(|c| {
        if let Command::Task(crate::tui::commands::TaskCommand::BatchPatchSubStatus { updates }) = c
        {
            updates.iter().any(|(id, _)| *id == TaskId(4))
        } else {
            false
        }
    }));
}

#[test]
fn window_gone_on_running_task_marks_crashed() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::WindowGone(TaskId(4)),
    ));
    assert!(app.is_crashed(TaskId(4)));
    // tmux_window should be cleared — the window is gone by definition
    assert!(app.board.tasks[0].tmux_window.is_none());
    // Should emit PersistTask with cleared tmux_window
    assert!(cmds.iter().any(
        |c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Persist(t)) if t.id == TaskId(4) && t.tmux_window.is_none())
    ));
}

#[test]
fn window_gone_on_review_task_clears_window() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Review)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::WindowGone(TaskId(4)),
    ));
    assert!(!app.is_crashed(TaskId(4)));
    assert!(app.board.tasks[0].tmux_window.is_none());
    assert!(matches!(
        &cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    ));
}

#[test]
fn dispatched_sets_fields_and_transitions_to_running() {
    let mut task = make_task(3, TaskStatus::Backlog);
    task.plan_path = Some("plan.md".into());
    let mut app = App::new(vec![task]);
    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::Dispatched {
            id: TaskId(3),
            worktree: "/wt".to_string(),
            tmux_window: "win".to_string(),
            switch_focus: false,
        },
    ));
    let task = app.board.tasks.iter().find(|t| t.id == TaskId(3)).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/wt"));
    assert_eq!(task.tmux_window.as_deref(), Some("win"));
    // Stamped so ClassifyAgentActivity sees a recent PreToolUse and
    // does not flicker the freshly dispatched task into Stale.
    let stamped = task.last_pre_tool_use_at.expect("last_pre_tool_use_at set");
    assert!(
        chrono::Utc::now()
            .signed_duration_since(stamped)
            .num_seconds()
            < 5
    );
    // Persist plus the trailing repo-sync refresh
    // (docs/specs/repo-sync.allium: RefreshRepoSyncStateAfterDispatch).
    // No SeedActivity: the pre-provisioning claim already wrote the activity
    // stamp, so re-writing it here would be redundant — and would clobber a
    // real hook stamp if this handling ever ran later than it does.
    assert_eq!(cmds.len(), 2, "got: {cmds:?}");
    assert!(matches!(
        &cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    ));
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::SeedActivity { .. })
        )),
        "the claim owns the activity stamp on the dispatch path"
    );
}

#[test]
fn dispatched_with_switch_focus_emits_jump() {
    let mut task = make_task(3, TaskStatus::Backlog);
    task.plan_path = Some("plan.md".into());
    let mut app = App::new(vec![task]);
    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::Dispatched {
            id: TaskId(3),
            worktree: "/wt".to_string(),
            tmux_window: "win".to_string(),
            switch_focus: true,
        },
    ));
    // Persist, JumpToTmux, and the trailing repo-sync refresh.
    assert_eq!(cmds.len(), 3, "got: {cmds:?}");
    assert!(matches!(
        &cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    ));
    assert!(
        matches!(&cmds[1], Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { window }) if window == "win")
    );
}

#[test]
fn dispatched_unknown_id_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::Dispatched {
            id: TaskId(999),
            worktree: "/wt".to_string(),
            tmux_window: "win".to_string(),
            switch_focus: false,
        },
    ));
    assert!(cmds.is_empty());
    assert_eq!(app.board.tasks[0].status, TaskStatus::Backlog);
}

#[test]
fn kill_and_retry_enters_confirm_mode() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());
    app.board.tasks[0].sub_status = SubStatus::Stale;

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::KillAndRetry(TaskId(4)),
    ));
    assert!(matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn retry_resume_emits_kill_and_resume() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());
    app.board.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.board.tasks[0].sub_status = SubStatus::Stale;
    // Pretend this stale task's last activity was 5 minutes ago — well past
    // ACTIVE_THRESHOLD. Without the seed in handle_retry_resume, a tick that
    // fires before the Resumed message arrives would flip the task back to
    // Stale on the basis of this old timestamp.
    app.board.tasks[0].last_pre_tool_use_at =
        Some(chrono::Utc::now() - chrono::Duration::minutes(5));
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::RetryResume(TaskId(4)),
    ));

    // After retry resume, sub_status is no longer stale/crashed
    assert!(!app.is_stale(TaskId(4)));
    assert!(!app.is_crashed(TaskId(4)));
    // last_pre_tool_use_at must be seeded so the tick classifier sees a
    // fresh activity stamp through the ACTIVE_THRESHOLD window.
    let stamped = app.board.tasks[0]
        .last_pre_tool_use_at
        .expect("last_pre_tool_use_at seeded on retry resume");
    assert!(
        chrono::Utc::now()
            .signed_duration_since(stamped)
            .num_seconds()
            < 5,
        "expected fresh seed, got {stamped}"
    );
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::KillTmuxWindow { .. })
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Resume { .. })
    )));
}

#[test]
fn retry_fresh_emits_cleanup_and_dispatch() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());
    app.board.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.board.tasks[0].sub_status = SubStatus::Stale;
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::RetryFresh(TaskId(4)),
    ));

    assert!(!app.is_stale(TaskId(4)));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert_eq!(app.board.tasks[0].status, TaskStatus::Backlog);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    )));
}

/// #4096 on the retry path: a crashed task whose worktree pointer is already
/// clear still owns a window, and re-dispatching over it must reclaim it rather
/// than leave a second window behind
/// (`TeardownIsOwedWheneverThereIsSomethingToRelease`).
#[test]
fn retry_fresh_tears_down_a_window_with_no_worktree() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());
    app.board.tasks[0].worktree = None;
    app.board.tasks[0].sub_status = SubStatus::Crashed;
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::RetryFresh(TaskId(4)),
    ));

    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Cleanup {
                worktree: None,
                tmux_window: Some(w),
                ..
            }) if w == "task-4"
        )),
        "the stale window must be torn down before the re-dispatch, got: {cmds:?}"
    );
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    )));
}

#[test]
fn crashed_card_with_no_window_shows_detached_not_crashed() {
    // Detached out-prioritizes Crashed when tmux_window is None
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Crashed;
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "○ detached"), "expected '○ detached'");
    assert!(
        !buffer_contains(&buf, "\u{26a0} crashed"),
        "should not show ⚠ crashed"
    );
}

/// `@guarantee ShownForWindowlessRunning` in docs/specs/dispatch.allium — a
/// Running task with neither worktree nor window is otherwise indistinguishable
/// from a healthy live agent.
#[test]
fn unprovisioned_running_card_shows_no_worktree() {
    let task = make_unprovisioned_task(1, TaskStatus::Running);
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "\u{26a0} no worktree"),
        "expected '⚠ no worktree'"
    );
    assert!(
        !buffer_contains(&buf, "\u{25c9} running"),
        "should not render as an ordinary running card"
    );
}

#[test]
fn quick_dispatch_zero_is_noop() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('0')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_non_digit_is_noop() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('a')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn resumed_seeds_last_pre_tool_use_at() {
    // ResumeTask spec ensures last_pre_tool_use_at = now so the resumed
    // task is classified Active until the agent's first PreToolUse hook,
    // matching DispatchTask behaviour.
    let mut t = make_task(1, TaskStatus::Running);
    t.worktree = Some("/repo/.worktrees/1-t".to_string());
    let mut app = App::new(vec![t]);
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Resumed {
        id: TaskId(1),
        tmux_window: "task-1".to_string(),
    }));
    let task = app.find_task(TaskId(1)).unwrap();
    let stamp = task
        .last_pre_tool_use_at
        .expect("resume should seed last_pre_tool_use_at");
    assert!(
        chrono::Utc::now()
            .signed_duration_since(stamp)
            .num_seconds()
            < 5
    );
    assert!(matches!(
        &cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    ));
}

// `TaskMessage::FinishFailed` no longer exists — the TUI wrap-up entry
// point (`W`) is gone, so the board never learns about a rebase conflict
// via a TUI message. The MCP `wrap_up` tool sets `sub_status` directly via
// `TaskService`, and the board picks it up on its next DB refresh. These
// tests set the conflict flag directly to model that refresh.

#[test]
fn conflict_flag_clears_on_dispatch() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }]);
    app.find_task_mut(TaskId(1)).unwrap().sub_status = SubStatus::Conflict;
    assert!(app
        .find_task(TaskId(1))
        .is_some_and(|t| t.sub_status == SubStatus::Conflict));

    app.update(Message::Task(crate::tui::messages::TaskMessage::Resumed {
        id: TaskId(1),
        tmux_window: "task-1".to_string(),
    }));
    assert!(!app
        .find_task(TaskId(1))
        .is_some_and(|t| t.sub_status == SubStatus::Conflict));
}

#[test]
fn conflict_flag_clears_on_move_backward() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }]);
    app.find_task_mut(TaskId(1)).unwrap().sub_status = SubStatus::Conflict;

    app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(1),
        direction: MoveDirection::Backward,
    }));
    assert!(!app
        .find_task(TaskId(1))
        .is_some_and(|t| t.sub_status == SubStatus::Conflict));
}

#[test]
fn dispatch_is_noop_when_on_select_all() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('k')));
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('d'))));
    assert!(cmds.is_empty());
}

#[test]
fn pr_merged_moves_to_done_and_detaches() {
    let mut task = make_task(1, TaskStatus::Review);
    task.tmux_window = Some("task-1".to_string());
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);
    app.set_notifications_enabled(true);

    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::Merged(
        TaskId(1),
    )));

    let task = app.find_task(TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(task.tmux_window.is_none(), "tmux window should be cleared");
    assert!(task.worktree.is_some(), "worktree should be preserved");
    assert!(task.url.is_some(), "url should be preserved");
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
    )));
}

#[test]
fn pr_merged_preserves_worktree() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::Merged(
        TaskId(1),
    )));

    // Should NOT emit a Cleanup command
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Cleanup { .. })
    )));
}

#[test]
fn pr_closed_moves_to_done_and_detaches() {
    let mut task = make_task(1, TaskStatus::Review);
    task.tmux_window = Some("task-1".to_string());
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);
    app.set_notifications_enabled(true);

    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::Closed(
        TaskId(1),
    )));

    let task = app.find_task(TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(task.tmux_window.is_none(), "tmux window should be cleared");
    assert!(task.worktree.is_some(), "worktree should be preserved");
    assert!(task.url.is_some(), "url should be preserved");
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
    )));
}

#[test]
fn pr_closed_status_message_says_closed_not_merged() {
    let mut task = make_task(1, TaskStatus::Review);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);

    app.update(Message::Pr(crate::tui::messages::PrMessage::Closed(
        TaskId(1),
    )));

    let status = app.status_message().unwrap_or_default();
    assert!(
        status.contains("closed"),
        "expected 'closed' in status bar, got: {status}"
    );
    assert!(
        !status.contains("merged"),
        "status bar should not say 'merged' for a closed PR, got: {status}"
    );
}

#[test]
fn pr_closed_no_notification_when_disabled() {
    let mut task = make_task(1, TaskStatus::Review);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);
    // notifications_enabled is false by default in tests

    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::Closed(
        TaskId(1),
    )));

    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
    )));
}

#[test]
fn pr_closed_ignores_non_review_task() {
    let task = make_task(1, TaskStatus::Done);
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::Closed(
        TaskId(1),
    )));

    let task = app.find_task(TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done, "status should be unchanged");
    assert!(cmds.is_empty(), "no commands expected for non-review task");
}

#[test]
fn pr_polling_skips_done_tasks() {
    let mut task = make_task(1, TaskStatus::Done);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    // Should NOT contain any CheckPrStatus command
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Pr(crate::tui::commands::PrCommand::CheckStatus { .. })
    )));
}

#[test]
fn pr_polling_emits_check_for_review_tasks() {
    let mut task = make_task(1, TaskStatus::Review);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    assert!(cmds.iter().any(|c| matches!(c, Command::Pr(crate::tui::commands::PrCommand::CheckStatus { ref url, .. }) if url == "https://github.com/org/repo/pull/42")));
}

#[test]
fn pr_polling_only_targets_pr_typed_urls() {
    // PR-typed review task — should be polled.
    let mut pr_task = make_task(1, TaskStatus::Review);
    pr_task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    // Issue-typed review task — must NOT be polled.
    let mut issue_task = make_task(2, TaskStatus::Review);
    issue_task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/issues/7",
        crate::models::UrlType::Issue,
    ));
    // Review task with no url — must NOT be polled.
    let url_less_task = make_task(3, TaskStatus::Review);

    let mut app = App::new(vec![pr_task, issue_task, url_less_task]);

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));

    let polled_ids: Vec<TaskId> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::Pr(crate::tui::commands::PrCommand::CheckStatus { id, .. }) => Some(*id),
            _ => None,
        })
        .collect();

    assert_eq!(polled_ids, vec![TaskId(1)]);
}

#[test]
fn tick_reclassifies_running_task_to_stale_when_pre_tool_use_is_old() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());
    app.board.tasks[0].sub_status = SubStatus::Active;
    let old = chrono::Utc::now() - ACTIVE_THRESHOLD - chrono::Duration::seconds(5);
    app.board.tasks[0].last_pre_tool_use_at = Some(old);

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Stale);
    assert!(cmds.iter().any(|c| {
        if let Command::Task(crate::tui::commands::TaskCommand::BatchPatchSubStatus { updates }) = c
        {
            updates
                .iter()
                .any(|(id, ss)| *id == TaskId(3) && *ss == SubStatus::Stale)
        } else {
            false
        }
    }));
}

#[test]
fn tick_reclassifies_running_task_to_needs_input_when_notification_newer() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());
    app.board.tasks[0].sub_status = SubStatus::Active;
    let now = chrono::Utc::now();
    app.board.tasks[0].last_pre_tool_use_at = Some(now - chrono::Duration::seconds(30));
    app.board.tasks[0].last_notification_at = Some(now - chrono::Duration::seconds(5));

    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::NeedsInput);
}

#[test]
fn tick_does_not_overwrite_crashed() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());
    app.board.tasks[0].sub_status = SubStatus::Crashed;
    let old = chrono::Utc::now() - chrono::Duration::minutes(5);
    app.board.tasks[0].last_pre_tool_use_at = Some(old);

    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Crashed);
}

#[test]
fn tick_does_not_overwrite_conflict() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());
    app.board.tasks[0].sub_status = SubStatus::Conflict;
    let old = chrono::Utc::now() - chrono::Duration::minutes(5);
    app.board.tasks[0].last_pre_tool_use_at = Some(old);

    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Conflict);
}

#[test]
fn crashed_detection_sets_substatus_and_persists() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::AgentCrashed(TaskId(3)),
    ));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::Crashed);
    assert!(cmds
        .iter()
        .any(|c| matches!(c, Command::Task(crate::tui::commands::TaskCommand::Persist(t)) if t.id == TaskId(3))));
}

#[test]
fn crash_emits_a_non_draining_subagent_clear() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Running)]);
    if let Some(t) = app.find_task_mut(TaskId(1)) {
        t.live_subagents = 2;
        t.stop_pending = true;
    }

    let cmds = app.handle_agent_crashed(TaskId(1));

    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::ClearSubagents {
                id,
                mode: crate::models::DrainMode::NoDrain,
            }) if *id == TaskId(1)
        )),
        "a crash must clear subagents without draining to Review"
    );
    assert_eq!(
        app.find_task(TaskId(1)).unwrap().live_subagents,
        0,
        "the board repaints immediately, without waiting for the DB round trip"
    );
    assert!(!app.find_task(TaskId(1)).unwrap().stop_pending);
}

#[test]
fn crashed_skips_non_running_task() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Review)]);

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::AgentCrashed(TaskId(3)),
    ));
    let task = app.find_task(TaskId(3)).unwrap();
    assert_eq!(task.sub_status, SubStatus::AwaitingReview); // unchanged
    assert!(cmds.is_empty());
}

#[test]
fn crashed_notification_sent_urgent_when_enabled() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());
    app.set_notifications_enabled(true);

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::AgentCrashed(TaskId(3)),
    ));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::System(crate::tui::commands::SystemCommand::SendNotification { urgent: true, .. })
    )));
}

#[test]
fn crashed_notification_not_sent_when_disabled() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("win-3".to_string());
    app.set_notifications_enabled(false);

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::AgentCrashed(TaskId(3)),
    ));
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
    )));
}

#[test]
fn pr_review_state_updates_substatus() {
    let mut app = make_app();
    let id = TaskId(3);
    app.find_task_mut(id).unwrap().status = TaskStatus::Review;
    app.find_task_mut(id).unwrap().sub_status = SubStatus::AwaitingReview;
    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::ReviewState {
        id,
        review_decision: Some(ReviewDecision::Approved),
    }));
    let task = app.find_task(id).unwrap();
    assert_eq!(task.sub_status, SubStatus::Approved);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[test]
fn pr_review_state_noop_when_unchanged() {
    let mut app = make_app();
    let id = TaskId(3);
    app.find_task_mut(id).unwrap().status = TaskStatus::Review;
    app.find_task_mut(id).unwrap().sub_status = SubStatus::AwaitingReview;
    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::ReviewState {
        id,
        review_decision: None, // maps to AwaitingReview
    }));
    assert!(cmds.is_empty()); // no change, no persist
}

#[test]
fn pr_review_state_changes_requested() {
    let mut app = make_app();
    let id = TaskId(3);
    app.find_task_mut(id).unwrap().status = TaskStatus::Review;
    app.find_task_mut(id).unwrap().sub_status = SubStatus::AwaitingReview;
    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::ReviewState {
        id,
        review_decision: Some(ReviewDecision::ChangesRequested),
    }));
    let task = app.find_task(id).unwrap();
    assert_eq!(task.sub_status, SubStatus::ChangesRequested);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[test]
fn pr_review_state_ignores_non_review_task() {
    let mut app = make_app();
    let id = TaskId(3);
    // Task 3 is Running by default in make_app
    assert_eq!(app.find_task(id).unwrap().status, TaskStatus::Running);
    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::ReviewState {
        id,
        review_decision: Some(ReviewDecision::Approved),
    }));
    assert!(cmds.is_empty());
    // sub_status should not have changed
    assert_ne!(app.find_task(id).unwrap().sub_status, SubStatus::Approved);
}

#[test]
fn pr_review_state_preserves_conflict_substatus() {
    let mut app = make_app();
    let id = TaskId(3);
    app.find_task_mut(id).unwrap().status = TaskStatus::Review;
    app.find_task_mut(id).unwrap().sub_status = SubStatus::Conflict;
    let cmds = app.update(Message::Pr(crate::tui::messages::PrMessage::ReviewState {
        id,
        review_decision: Some(ReviewDecision::Approved),
    }));
    assert!(cmds.is_empty());
    assert_eq!(app.find_task(id).unwrap().sub_status, SubStatus::Conflict);
}

#[test]
fn quick_dispatch_down_arrow_moves_cursor_down() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 0;
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.input.repo_cursor, 1);
}

#[test]
fn quick_dispatch_j_typed_into_filter_buffer() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/jkl".to_string(), "/abc".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 0;
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.input.buffer, "j");
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn quick_dispatch_enter_selects_cursor_repo() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec![
        "/repo1".to_string(),
        "/repo2".to_string(),
        "/repo3".to_string(),
    ];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 2; // third repo
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, epic_id: None }) if draft.repo_path == "/repo3")
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn quick_dispatch_clears_buffer_on_entry() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    app.input.set_buffer("leftover".to_string());
    app.update(Message::Input(
        crate::tui::messages::InputMessage::StartQuickDispatchSelection,
    ));
    assert_eq!(app.input.buffer, "");
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_typing_updates_buffer() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/api-service".to_string(), "/frontend".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.handle_key(make_key(KeyCode::Char('a')));
    assert_eq!(app.input.buffer, "a");
}

#[test]
fn quick_dispatch_typing_resets_cursor() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec![
        "/api".to_string(),
        "/backend".to_string(),
        "/core".to_string(),
    ];
    app.input.mode = InputMode::QuickDispatch;
    app.input.repo_cursor = 2;
    app.handle_key(make_key(KeyCode::Char('a')));
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn quick_dispatch_backspace_shrinks_buffer_and_resets_cursor() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/api".to_string(), "/backend".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.set_buffer("ap".to_string());
    app.input.repo_cursor = 1;
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "a");
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn quick_dispatch_enter_selects_from_filtered_list() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec![
        "/api-service".to_string(),
        "/backend".to_string(),
        "/api-gateway".to_string(),
    ];
    app.input.mode = InputMode::QuickDispatch;
    app.input.set_buffer("api".to_string()); // matches index 0 and 2 of full list
    app.input.repo_cursor = 1; // second item in filtered list → "/api-gateway"
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, .. }) if draft.repo_path == "/api-gateway")
    );
}

#[test]
fn quick_dispatch_enter_uses_buffer_as_new_repo_when_no_match() {
    // When the typed path matches no existing repos, Enter should dispatch
    // to the literal buffer value as a brand-new repo path.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/api-service".to_string(), "/backend".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input
        .set_buffer("/home/user/brand-new-project".to_string());
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, .. })
            if draft.repo_path == "/home/user/brand-new-project")
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn quick_dispatch_enter_uses_buffer_when_cursor_on_new_entry() {
    // When the buffer fuzzy-matches an existing repo but the cursor is on the
    // trailing "new path" entry, Enter dispatches with the raw buffer value.
    // repos = ["/home/code/project-work"]; buffer = "/home/code/work"
    // filtered = ["/home/code/project-work"] (fuzzy match), new entry at idx 1
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec!["/home/code/project-work".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.set_buffer("/home/code/work".to_string());
    app.input.repo_cursor = 1; // cursor on new entry (past the 1 filtered result)
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, .. })
            if draft.repo_path == "/home/code/work")
    );
}

#[test]
fn quick_dispatch_cursor_navigates_to_new_entry() {
    // Down arrow should be able to move the cursor past the filtered list to
    // the new-path entry when the buffer is non-empty and not an exact match.
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/home/code/project-work".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.set_buffer("/home/code/work".to_string()); // fuzzy-matches repo, but not exact
    app.input.repo_cursor = 0;
    app.handle_key(make_key(KeyCode::Down));
    // Should have moved to index 1 (the new-path entry)
    assert_eq!(app.input.repo_cursor, 1);
}

#[test]
fn quick_dispatch_no_new_entry_when_buffer_exactly_matches_repo() {
    // When the buffer is an exact match for an existing repo path, there
    // is no new-entry slot, so Down wraps within the filtered list.
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repo/a".to_string(), "/repo/b".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.set_buffer("/repo/a".to_string()); // exact match → no new entry
                                                 // filtered = ["/repo/a"] only (only exact match on the query chars)
    app.input.repo_cursor = 0;
    app.handle_key(make_key(KeyCode::Down));
    // filtered has 1 item, no new entry → wraps back to 0
    assert_eq!(app.input.repo_cursor, 0);
}

#[test]
fn quick_dispatch_enter_uses_cursor_within_filtered_list() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.repo_paths = vec![
        "/api-service".to_string(),
        "/backend".to_string(),
        "/api-gateway".to_string(),
    ];
    app.input.mode = InputMode::QuickDispatch;
    app.input.set_buffer("api".to_string()); // filtered: ["/api-service", "/api-gateway"]
    app.input.repo_cursor = 1; // pick second filtered item → "/api-gateway"
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Enter)));
    assert_eq!(cmds.len(), 1);
    assert!(
        matches!(&cmds[0], Command::Task(crate::tui::commands::TaskCommand::QuickDispatch { ref draft, .. }) if draft.repo_path == "/api-gateway")
    );
}

#[test]
fn tab_hint_absent_from_tab_bar() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 100, 30);
    assert!(
        find_style_of(&buf, "[Tab]").is_none(),
        "[Tab] hint must not appear in the board view"
    );
}

#[test]
fn dispatch_in_flight_blocks_second_dispatch() {
    let mut app = make_app();
    // First dispatch succeeds
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));
    assert!(matches!(
        cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    ));
    // Second dispatch of same task is blocked
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));
    assert!(cmds.is_empty());
}

#[test]
fn dispatched_clears_in_flight() {
    let mut app = make_app();
    // Dispatch task 1
    app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));
    // Dispatched message clears the in-flight guard
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::Dispatched {
            id: TaskId(1),
            worktree: "/wt".to_string(),
            tmux_window: "win".to_string(),
            switch_focus: false,
        },
    ));
    // Task is now Running, so dispatch is a no-op for a different reason,
    // but the in-flight set should be clear
    assert!(!app.is_dispatching(TaskId(1)));
}

#[test]
fn dispatch_failed_clears_in_flight() {
    let mut app = make_app();
    // Dispatch task 1
    app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));
    assert!(app.is_dispatching(TaskId(1)));
    // DispatchFailed clears the in-flight guard
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::DispatchFailed(TaskId(1)),
    ));
    assert!(!app.is_dispatching(TaskId(1)));
    // Can dispatch again
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));
    assert!(matches!(
        cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    ));
}

#[test]
fn dispatch_different_tasks_both_succeed() {
    let mut app = make_app();
    // Dispatch task 1
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));
    assert!(matches!(
        cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    ));
    // Dispatch task 2 — different task, should succeed
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(2),
        DispatchMode::Dispatch,
    )));
    assert!(matches!(
        cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    ));
}

#[test]
fn dispatch_failed_clears_mark_dispatching_guard() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    assert!(app.is_dispatching(TaskId(1)));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::DispatchFailed(TaskId(1)),
    ));
    assert!(!app.is_dispatching(TaskId(1)));
}

/// Every dispatch entry point claims before it provisions, so every way a
/// dispatch can end without provisioning owes the claim back. `DispatchFailed`
/// is the single funnel for all of them — the failed and panicked dispatch arms
/// and the failed repo-trust grant — so the release rides on it rather than
/// being re-derived at each producer.
#[test]
fn dispatch_failed_releases_the_claim() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::DispatchFailed(TaskId(1)),
    ));

    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::ReleaseClaim(id)) if *id == TaskId(1)
        )),
        "DispatchFailed must release the claim, got: {cmds:?}"
    );
}

/// A *lost* claim must NOT release, and so cannot travel on `DispatchFailed`.
///
/// The winner of a contested claim is itself Running with no worktree while it
/// provisions — exactly the state `release_claim` is conditional on. So a loser
/// that released would hand the winner's task back to Backlog mid-provision,
/// leaving the winner to patch a worktree onto a task the board thinks is
/// dispatchable and inviting a third dispatch of the same work. The loser owns
/// no claim, so it drains its spinner and stops there.
#[test]
fn dispatch_claim_lost_clears_the_spinner_without_releasing() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::DispatchAbandoned(TaskId(1)),
    ));

    assert!(
        !app.is_dispatching(TaskId(1)),
        "a lost claim must still drain the spinner"
    );
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::ReleaseClaim(_))
        )),
        "the loser owns no claim and must not release the winner's, got: {cmds:?}"
    );
}

/// `DISPATCH_WATCHDOG_TIMEOUT` must be derived from `provision_worktree`'s
/// real worst-case subprocess budget (#4201), not mirror `SUBPROCESS_TIMEOUT`
/// 1:1 — the old 1:1 relationship is exactly the bug: a fetch legitimately
/// retrying within `FetchPolicy::Required`'s budget could still exceed a
/// watchdog sized for a single subprocess call.
#[test]
fn dispatch_watchdog_timeout_matches_provision_worst_case() {
    assert_eq!(
        crate::tui::DISPATCH_WATCHDOG_TIMEOUT,
        crate::process::SUBPROCESS_TIMEOUT * crate::dispatch::PROVISION_MAX_SUBPROCESS_CALLS,
        "the watchdog must be sized off SUBPROCESS_TIMEOUT * PROVISION_MAX_SUBPROCESS_CALLS \
         (PROVISION_MAX_SUBPROCESS_CALLS > 1, so this also rules out the old 1:1 mirror — \
         exactly the bug #4201 fixed: fetch_origin's retry budget under FetchPolicy::Required \
         can issue several SUBPROCESS_TIMEOUT-bounded calls before a fresh dispatch succeeds \
         or gives up)"
    );
}

/// The watchdog must NOT release, even though it drains `dispatching`.
///
/// "Slower than the deadline" is not "dead" — a `git fetch` on a slow network outlives the
/// deadline with its worker perfectly alive. Releasing would return the task to
/// Backlog mid-provision and let a second dispatch land on the same branch, which
/// is the double-provisioning `DispatchClaimExclusive` rules out. The watchdog
/// stays a UI backstop: it clears the spinner and says so. A truly dead worker
/// leaves the task Running with no worktree, which renders as detached and is
/// recoverable via retry-fresh.
#[test]
fn dispatching_timeout_does_not_release_the_claim() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    app.dispatching.insert(
        TaskId(1),
        Instant::now() - crate::tui::DISPATCH_WATCHDOG_TIMEOUT - Duration::from_secs(1),
    );

    let cmds = app.update(Message::System(crate::tui::messages::SystemMessage::Tick));

    assert!(
        !app.is_dispatching(TaskId(1)),
        "the watchdog still drains the spinner"
    );
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::ReleaseClaim(_))
        )),
        "a slow-but-alive dispatch must keep its claim, got: {cmds:?}"
    );
}

/// The claim flips the task to Running before provisioning finishes, so the
/// "dispatching…" indicator has to survive that transition. Before the claim
/// existed, a member of `dispatching` was always Backlog; now it is Backlog
/// (pre-claim) or Running-without-worktree (claimed, being provisioned), and the
/// indicator must win over the status-derived ones in both cases or an in-flight
/// dispatch would render as a live agent (`SpansTheClaim` in
/// docs/specs/dispatch.allium).
#[test]
fn claimed_task_still_renders_the_dispatching_indicator() {
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = None;
    task.tmux_window = None;
    let mut app = App::new(vec![task]);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));

    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "dispatching"),
        "a claimed-but-unprovisioned task must still show 'dispatching', not a running indicator"
    );
}

#[test]
fn window_gone_ignored_for_split_pinned_task() {
    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task]);

    // Pin task 4 in split mode
    app.board.split.active = true;
    app.board.split.right_pane_id = Some("%42".to_string());
    app.board.split.pinned_task_id = Some(TaskId(4));

    // Even if WindowGone fires for the pinned task, it should NOT crash
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::WindowGone(TaskId(4)),
    ));
    assert!(
        !app.is_crashed(TaskId(4)),
        "split-pinned task should not be marked as crashed"
    );
}

// ---------------------------------------------------------------------------
// Sticky dispatching status — handler-level coverage (Task #500)
// ---------------------------------------------------------------------------

#[test]
fn handle_dispatch_task_sets_sticky_status() {
    let mut app = make_app(); // task 1 is Backlog with title "Task 1"
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));

    // The DispatchAgent command is still produced.
    assert!(matches!(
        cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    ));
    // And the sticky status is set so the user sees feedback while
    // git fetch / worktree creation runs in spawn_blocking.
    let msg = app.status.message.as_deref().expect("sticky status set");
    assert!(msg.contains("Dispatching"), "got: {msg}");
    assert!(msg.contains("Task 1"), "got: {msg}");
    assert!(app.status.message_sticky);
    assert!(app.is_dispatching(TaskId(1)));
}

#[test]
fn handle_dispatch_task_rejects_when_already_dispatching() {
    let mut app = make_app();
    // First dispatch sets the in-flight guard
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));
    assert!(matches!(
        cmds[0],
        Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
    ));

    // Second dispatch of the same task is debounced — no new command.
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Dispatch(
        TaskId(1),
        DispatchMode::Dispatch,
    )));
    assert!(cmds.is_empty(), "second dispatch should be rejected");
    // Sticky status remains set (we're still in flight).
    assert!(app.status.message_sticky);
}

#[test]
fn dispatching_timeout_clears_stuck_task() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    assert!(app.is_dispatching(TaskId(1)));

    // Backdate the start time past the watchdog deadline.
    app.dispatching.insert(
        TaskId(1),
        Instant::now() - crate::tui::DISPATCH_WATCHDOG_TIMEOUT - Duration::from_secs(1),
    );

    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));

    assert!(
        !app.is_dispatching(TaskId(1)),
        "watchdog should remove a task that has been mid-dispatch past the deadline"
    );
    let popup = app
        .status
        .error_popup
        .as_deref()
        .expect("watchdog should set an error popup");
    assert!(
        popup.contains("timed out") || popup.contains("timeout"),
        "expected timeout-related error popup, got: {popup}"
    );
}

#[test]
fn dispatching_start_time_recorded_on_mark() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    assert!(
        app.dispatching.contains_key(&TaskId(1)),
        "mark_dispatching should record the start time for the watchdog"
    );
}

#[test]
fn dispatching_start_time_cleared_on_dispatched() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    assert!(app.dispatching.contains_key(&TaskId(1)));

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::Dispatched {
            id: TaskId(1),
            worktree: "/wt".to_string(),
            tmux_window: "win-1".to_string(),
            switch_focus: false,
        },
    ));

    assert!(
        !app.dispatching.contains_key(&TaskId(1)),
        "Dispatched should remove the task from the dispatching map"
    );
}

#[test]
fn dispatching_card_renders_with_indicator_text() {
    let mut app = make_app(); // task 1 is Backlog
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));

    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "dispatching"),
        "Backlog card for task 1 should show the 'dispatching' indicator while in flight"
    );
}

#[test]
fn spinner_tick_advances_only_on_tick_when_dispatching_nonempty() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    let before = app.spinner_tick;

    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));

    assert_ne!(
        app.spinner_tick, before,
        "spinner_tick should advance on Tick while dispatching is non-empty"
    );
}

#[test]
fn spinner_tick_does_not_advance_when_dispatching_empty() {
    let mut app = make_app();
    let before = app.spinner_tick;

    app.update(Message::System(crate::tui::messages::SystemMessage::Tick));

    assert_eq!(
        app.spinner_tick, before,
        "spinner_tick should stay frozen on Tick when dispatching is empty"
    );
}

#[test]
fn spinner_tick_wraps_modulo_ten() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));

    // 10 ticks should bring us back to the starting frame.
    let start = app.spinner_tick;
    for _ in 0..10 {
        app.update(Message::System(crate::tui::messages::SystemMessage::Tick));
    }
    assert_eq!(
        app.spinner_tick, start,
        "spinner_tick should wrap mod 10 so the rendered glyph cycles"
    );
}

#[test]
fn quick_dispatch_status_uses_freshly_created_title() {
    // Quick dispatch sends TaskCreated *then* MarkDispatching. The status
    // helper must look up the freshly-created task by ID — silently
    // reordering those two messages would silently break the title.
    let mut app = App::new(vec![]);
    let now = chrono::Utc::now();
    let task = Task {
        id: TaskId(42),
        title: "Quick task".to_string(),
        description: String::new(),
        repo_path: "/repo".to_string(),
        status: TaskStatus::Backlog,
        worktree: None,
        tmux_window: None,
        plan_path: None,
        epic_id: None,
        sub_status: SubStatus::default_for(TaskStatus::Backlog),
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

    app.update(Message::Task(crate::tui::messages::TaskMessage::Created {
        task: Box::new(task),
    }));
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(42)),
    ));

    let msg = app.status.message.as_deref().expect("status set");
    assert!(msg.contains("Quick task"), "got: {msg}");
    assert!(app.is_dispatching(TaskId(42)));
}

#[test]
fn manual_move_review_to_running_seeds_last_pre_tool_use_at() {
    let mut task = make_task(1, TaskStatus::Review);
    task.last_pre_tool_use_at = None;
    task.last_notification_at = None;
    task.sub_status = SubStatus::AwaitingReview;
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(1),
        direction: MoveDirection::Backward,
    }));

    let t = app.find_task(TaskId(1)).expect("task");
    assert_eq!(t.status, TaskStatus::Running);
    assert!(t.last_pre_tool_use_at.is_some(), "seed missing");
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::SeedActivity { id: TaskId(1), .. })
        )),
        "no SeedActivity in {cmds:?}",
    );
}

#[test]
fn dispatch_space_on_backlog_task_emits_trust_check_command() {
    // The trust check (`is_repo_trusted`, a `~/.claude.json` read) is file
    // I/O and must not run inline on the key-handling path — it is deferred
    // to a Command the runtime executes via spawn_blocking. handle_key must
    // not decide dispatch-vs-confirm-trust itself; it only emits the check.
    let mut app = make_app();
    // Navigate to task 1 (Backlog)
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::CheckTrustAndDispatch {
                id: TaskId(1),
                ..
            })
        )),
        "expected CheckTrustAndDispatch command, got {cmds:?}"
    );
    // No dispatch and no mode change happen synchronously — both depend on
    // the check command's result, which only the runtime can produce.
    assert!(
        cmds.iter().all(|c| !matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
        )),
        "should not dispatch before the trust check runs"
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_y_in_trust_mode_emits_trust_and_dispatch() {
    let mut app = make_app();
    // Enter confirm mode directly to avoid depending on is_repo_trusted state
    app.input.mode = InputMode::ConfirmTrustRepo {
        task_id: TaskId(1),
        mode: DispatchMode::Dispatch,
    };
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('y'))));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::TrustAndDispatch { .. })
        )),
        "expected TrustAndDispatch command"
    );
}

#[test]
fn confirm_n_in_trust_mode_returns_to_normal() {
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmTrustRepo {
        task_id: TaskId(1),
        mode: DispatchMode::Dispatch,
    };
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(
        cmds.iter().all(|c| !matches!(c, Command::Task(_))),
        "expected no task command on cancel"
    );
}

#[test]
fn trust_and_dispatch_message_emits_trust_command() {
    let mut app = make_app();
    let cmds = without_usage(app.update(Message::Task(
        crate::tui::messages::TaskMessage::TrustAndDispatch {
            id: TaskId(1),
            mode: DispatchMode::Dispatch,
        },
    )));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::TrustAndDispatch { .. })
        )),
        "expected TrustAndDispatch command from message handler"
    );
}

#[test]
fn trust_check_untrusted_enters_confirm_trust_mode() {
    // What the runtime sends back when CheckTrustAndDispatch's spawn_blocking
    // read of ~/.claude.json finds the repo untrusted.
    let mut app = make_app();
    let cmds = without_usage(app.update(Message::Task(
        crate::tui::messages::TaskMessage::TrustCheckUntrusted {
            id: TaskId(1),
            mode: DispatchMode::Dispatch,
            repo_path: "/repo".to_string(),
        },
    )));
    assert!(cmds.is_empty());
    assert!(
        matches!(
            app.input.mode,
            InputMode::ConfirmTrustRepo {
                task_id: TaskId(1),
                ..
            }
        ),
        "expected ConfirmTrustRepo mode, got {:?}",
        app.input.mode
    );
    let msg = app.status.message.as_deref().expect("status set");
    assert!(msg.contains("/repo"), "got: {msg}");
}

#[test]
fn trust_check_untrusted_ignored_if_already_dispatching() {
    // Guards the async gap: if the task was dispatched (or otherwise no
    // longer a pending Backlog dispatch) by the time the check result
    // arrives, don't clobber whatever mode the UI is in now.
    let mut app = make_app();
    app.mark_dispatching(TaskId(1));
    app.input.mode = InputMode::Help;
    let cmds = without_usage(app.update(Message::Task(
        crate::tui::messages::TaskMessage::TrustCheckUntrusted {
            id: TaskId(1),
            mode: DispatchMode::Dispatch,
            repo_path: "/repo".to_string(),
        },
    )));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Help);
}

#[test]
fn confirm_y_in_trust_mode_emits_trust_and_quick_dispatch() {
    let mut app = make_app();
    let draft = TaskDraft {
        title: DEFAULT_QUICK_TASK_TITLE.to_string(),
        repo_path: "/my/repo".to_string(),
        ..Default::default()
    };
    app.input.mode = InputMode::ConfirmTrustRepoQuickDispatch {
        draft: draft.clone(),
        epic_id: None,
    };
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('y'))));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::TrustAndQuickDispatch {
                draft: d,
                epic_id: None,
            }) if d.repo_path == draft.repo_path
        )),
        "expected TrustAndQuickDispatch command, got {cmds:?}"
    );
}

#[test]
fn confirm_n_in_trust_mode_for_quick_dispatch_returns_to_normal() {
    let mut app = make_app();
    let draft = TaskDraft {
        title: DEFAULT_QUICK_TASK_TITLE.to_string(),
        repo_path: "/my/repo".to_string(),
        ..Default::default()
    };
    app.input.mode = InputMode::ConfirmTrustRepoQuickDispatch {
        draft,
        epic_id: None,
    };
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(
        cmds.iter().all(|c| !matches!(c, Command::Task(_))),
        "expected no task command on cancel"
    );
}

#[test]
fn trust_check_untrusted_for_quick_dispatch_enters_confirm_mode() {
    // What the runtime sends back when the QuickDispatch command's
    // spawn_blocking read of ~/.claude.json finds the repo untrusted.
    let mut app = make_app();
    let draft = TaskDraft {
        title: DEFAULT_QUICK_TASK_TITLE.to_string(),
        repo_path: "/my/repo".to_string(),
        ..Default::default()
    };
    let cmds = without_usage(app.update(Message::Task(
        crate::tui::messages::TaskMessage::TrustCheckUntrustedForQuickDispatch {
            draft: draft.clone(),
            epic_id: None,
        },
    )));
    assert!(cmds.is_empty());
    assert!(
        matches!(
            &app.input.mode,
            InputMode::ConfirmTrustRepoQuickDispatch { draft: d, epic_id: None }
            if d.repo_path == draft.repo_path
        ),
        "expected ConfirmTrustRepoQuickDispatch mode, got {:?}",
        app.input.mode
    );
    let msg = app.status.message.as_deref().expect("status set");
    assert!(msg.contains("/my/repo"), "got: {msg}");
}

// ---------------------------------------------------------------------------
// Unified Space "activate task" behavior (docs/specs/split-pane.allium:
// JumpToAgentWindow). Space jumps to a live window when one exists; otherwise
// it dispatches / resumes / opens the retry dialog. `d` is no longer bound.
// The window-present cases already jump under the pre-existing Space handler,
// so the behavioral change is confined to the no-window branches below.
// ---------------------------------------------------------------------------

#[test]
fn space_on_backlog_no_window_dispatches() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.tag = Some(TaskTag::Feature);
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(1); // Backlog column
    app.selection_mut().set_row(1, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    // Space on a windowless Backlog task routes through the repo-trust gate
    // (a CheckTrustAndDispatch command) rather than dispatching or showing
    // "No active session" synchronously.
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::CheckTrustAndDispatch {
                id: TaskId(1),
                ..
            })
        )),
        "expected CheckTrustAndDispatch command, got {cmds:?}"
    );
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn space_on_running_no_window_with_worktree_resumes() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(2); // Running column
    app.selection_mut().set_row(2, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Resume { .. })
        )),
        "Space on a windowless Running task with a worktree should resume, got {cmds:?}"
    );
}

#[test]
fn space_on_stale_running_no_window_enters_retry_mode() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = None;
    task.sub_status = SubStatus::Stale;
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(2); // Running column
    app.selection_mut().set_row(2, 0);
    app.handle_key(make_key(KeyCode::Char(' ')));
    // Windowless Stale/Crashed is the only path that still opens the retry dialog.
    assert!(
        matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))),
        "expected ConfirmRetry mode, got {:?}",
        app.input.mode
    );
}

#[test]
fn space_on_crashed_running_no_window_enters_retry_mode() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = None;
    task.sub_status = SubStatus::Crashed;
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(2); // Running column
    app.selection_mut().set_row(2, 0);
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(
        matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))),
        "expected ConfirmRetry mode, got {:?}",
        app.input.mode
    );
}

/// `@guarantee RetryReachableInPlace` in docs/specs/dispatch.allium — an
/// unprovisioned Running task offers kill-and-retry whatever its sub_status,
/// rather than a dead-end hint. The fixture's sub_status is the default
/// `Active`: the stale/crashed tick classifications both skip windowless
/// tasks, so such a task never reaches either.
#[test]
fn space_on_unprovisioned_running_enters_retry_mode() {
    let task = make_unprovisioned_task(4, TaskStatus::Running);
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(2); // Running column
    app.selection_mut().set_row(2, 0);
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(
        matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))),
        "expected ConfirmRetry mode, got {:?}",
        app.input.mode
    );
}

/// `@guarantee DispatchingOutranksIt` — the claim writes Running before the
/// worktree exists, so an in-flight task is unprovisioned by construction.
/// Offering retry there would move it back to Backlog and fire a SECOND
/// DispatchAgent alongside the one already running.
#[test]
fn space_on_dispatching_unprovisioned_running_does_not_offer_retry() {
    let task = make_unprovisioned_task(4, TaskStatus::Running);
    let mut app = App::new(vec![task]);
    app.mark_dispatching(TaskId(4));
    app.selection_mut().set_column(2); // Running column
    app.selection_mut().set_row(2, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        !matches!(app.input.mode, InputMode::ConfirmRetry(_)),
        "must not open the retry dialog mid-dispatch, got {:?}",
        app.input.mode
    );
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
        )),
        "must not fire a second dispatch, got {cmds:?}"
    );
    assert!(
        app.status
            .message
            .as_deref()
            .unwrap_or("")
            .contains("Dispatch in progress"),
        "expected an in-progress hint, got {:?}",
        app.status.message
    );
}

/// The epic auto-dispatch chain claims its subtask inside the MCP handler, so
/// it is never in `app.dispatching` — the map alone would leave a chained
/// subtask one keypress from a duplicate agent for its whole provisioning
/// window. A fresh claim stamp closes that hole.
#[test]
fn space_on_freshly_claimed_task_outside_dispatching_map_does_not_offer_retry() {
    let mut task = make_unprovisioned_task(4, TaskStatus::Running);
    task.last_pre_tool_use_at = Some(chrono::Utc::now() - chrono::Duration::seconds(5));
    let mut app = App::new(vec![task]);
    assert!(!app.is_dispatching(TaskId(4)), "not in the map");
    app.selection_mut().set_column(2); // Running column
    app.selection_mut().set_row(2, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        !matches!(app.input.mode, InputMode::ConfirmRetry(_)),
        "must not open the retry dialog while provisioning, got {:?}",
        app.input.mode
    );
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::DispatchAgent { .. })
        )),
        "must not fire a second dispatch, got {cmds:?}"
    );
}

/// The hint bar must agree with the key: no `[Space] retry` while in flight.
#[test]
fn action_hints_hide_retry_while_dispatch_in_flight() {
    let task = make_unprovisioned_task(4, TaskStatus::Running);
    let hints =
        crate::tui::ui::action_hints(Some(&task), true, ratatui::style::Color::Rgb(122, 162, 247));
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(!text.contains("retry"), "got {text:?}");
}

/// The dialog's `[r] Resume` branch dead-ends for an unprovisioned task (there
/// is no worktree to resume into), so it must not be advertised.
#[test]
fn retry_dialog_for_unprovisioned_offers_fresh_start_only() {
    let task = make_unprovisioned_task(4, TaskStatus::Running);
    let mut app = App::new(vec![task]);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::KillAndRetry(TaskId(4)),
    ));
    let msg = app.status.message.as_deref().unwrap_or("");
    assert!(msg.contains("[f] Fresh start"), "got {msg:?}");
    assert!(!msg.contains("[r] Resume"), "got {msg:?}");
    assert!(
        !msg.contains("stale") && !msg.contains("crashed"),
        "an unprovisioned task is neither stale nor crashed, got {msg:?}"
    );
}

/// The widening is scoped to Running: `RetryFresh` refuses every other status,
/// so a Review task keeps the hint rather than getting a dialog that no-ops.
#[test]
fn space_on_review_no_window_no_worktree_shows_no_worktree_hint() {
    let task = make_unprovisioned_task(5, TaskStatus::Review);
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(3); // Review column
    app.selection_mut().set_row(3, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(cmds.is_empty());
    assert!(
        !matches!(app.input.mode, InputMode::ConfirmRetry(_)),
        "Review has no retry path, got {:?}",
        app.input.mode
    );
    assert!(
        app.status
            .message
            .as_deref()
            .unwrap_or("")
            .contains("No worktree"),
        "expected a 'No worktree' hint, got {:?}",
        app.status.message
    );
}

#[test]
fn d_key_on_backlog_is_inert() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.tag = Some(TaskTag::Feature);
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(1); // Backlog column
    app.selection_mut().set_row(1, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('d'))));
    // `d` is unbound: it dispatches nothing and opens no confirmation.
    assert!(cmds.is_empty(), "`d` should emit no commands, got {cmds:?}");
    assert!(
        !matches!(app.input.mode, InputMode::ConfirmTrustRepo { .. }),
        "`d` should not open the trust prompt, got {:?}",
        app.input.mode
    );
}

// Window-present cases: Space jumps regardless of status or sub_status. The
// window check wins over the Stale/Crashed problematic check.

#[test]
fn space_on_stale_running_with_window_jumps() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());
    app.board.tasks[0].sub_status = SubStatus::Stale;
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { window }) if window == "task-4"
        )),
        "Space on a Stale task with a window should jump, not open retry, got {cmds:?}"
    );
    assert!(
        !matches!(app.input.mode, InputMode::ConfirmRetry(_)),
        "Space on a windowed Stale task must not open the retry dialog"
    );
}

#[test]
fn space_on_crashed_running_with_window_jumps() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)]);
    app.board.tasks[0].tmux_window = Some("task-4".to_string());
    app.board.tasks[0].sub_status = SubStatus::Crashed;
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { window }) if window == "task-4"
        )),
        "Space on a Crashed task with a window should jump, got {cmds:?}"
    );
    assert!(!matches!(app.input.mode, InputMode::ConfirmRetry(_)));
}

#[test]
fn space_on_review_with_window_jumps() {
    let mut task = make_task(5, TaskStatus::Review);
    task.tmux_window = Some("task-5".to_string());
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(3); // Review column
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { window }) if window == "task-5"
        )),
        "Space on a Review task with a window should jump, got {cmds:?}"
    );
}

#[test]
fn space_on_done_with_window_jumps() {
    let mut task = make_task(1, TaskStatus::Done);
    task.tmux_window = Some("task-1".to_string());
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(4); // Done column
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::JumpToTmux { window }) if window == "task-1"
        )),
        "Space on a Done task with a window should jump, got {cmds:?}"
    );
}

#[test]
fn space_on_review_no_window_with_worktree_resumes() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(3); // Review column
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Resume { .. })
        )),
        "Space on a windowless Review task with a worktree should resume, got {cmds:?}"
    );
}

#[test]
fn space_on_done_no_window_with_worktree_resumes() {
    let mut task = make_task(1, TaskStatus::Done);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task]);
    app.selection_mut().set_column(4); // Done column
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::Resume { .. })
        )),
        "Space on a windowless Done task with a worktree should resume, got {cmds:?}"
    );
}

#[test]
fn space_on_empty_column_is_noop() {
    let mut app = App::new(vec![]);
    app.selection_mut().set_column(1);
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char(' '))));
    assert!(cmds.is_empty());
}

// ---------------------------------------------------------------------------
// Auto-dispatch failure — SurfaceAutoDispatchFailure / AutoDispatchFailureIndicator
// in docs/specs/epics.allium
// ---------------------------------------------------------------------------

fn auto_dispatch_failed_msg(id: i64) -> Message {
    Message::Task(crate::tui::messages::TaskMessage::AutoDispatchFailed {
        task_id: TaskId(id),
        epic_id: EpicId(7),
        reason: "git fetch origin main failed".to_string(),
    })
}

/// The marker is what survives once the status message and the notification
/// have gone. Without it a chain that died overnight is indistinguishable from
/// an epic that simply ran out of subtasks.
#[test]
fn auto_dispatch_failure_marks_the_subtask() {
    let mut app = make_app();
    assert!(!app.auto_dispatch_failed(TaskId(1)));

    app.update(auto_dispatch_failed_msg(1));

    assert!(app.auto_dispatch_failed(TaskId(1)));
    assert!(
        !app.auto_dispatch_failed(TaskId(2)),
        "only the subtask that failed is marked"
    );
}

/// StatusMessageNamesTheSubtask: an operator watching the board must learn
/// which chain stopped, not merely that something went wrong.
#[test]
fn auto_dispatch_failure_sets_a_status_message_naming_the_task() {
    let mut app = make_app();

    app.update(auto_dispatch_failed_msg(1));

    let status = app.status_message().unwrap_or_default().to_string();
    assert!(
        status.contains("#1"),
        "status message must name the subtask, got: {status}"
    );
    assert!(
        status.contains("git fetch origin main failed"),
        "status message must carry the reason, got: {status}"
    );
}

/// Gated on `notifications_enabled`, exactly as NotifyNeedsInput / NotifyReview
/// are (docs/specs/agent-health.allium).
#[test]
fn auto_dispatch_failure_notifies_when_notifications_are_enabled() {
    let mut app = make_app();
    app.set_notifications_enabled(true);

    let cmds = app.update(auto_dispatch_failed_msg(1));

    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::System(crate::tui::commands::SystemCommand::SendNotification { urgent, .. })
                if *urgent
        )),
        "an auto-dispatch failure must raise an urgent notification, got: {cmds:?}"
    );
}

#[test]
fn auto_dispatch_failure_does_not_notify_when_notifications_are_disabled() {
    let mut app = make_app();
    app.set_notifications_enabled(false);

    let cmds = app.update(auto_dispatch_failed_msg(1));

    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
        )),
        "notifications are gated on the setting, got: {cmds:?}"
    );
    assert!(
        app.auto_dispatch_failed(TaskId(1)),
        "the board marker is not gated on the notification setting"
    );
}

/// PersistsUntilRedispatched: a retry is the resolution, so starting one clears
/// the marker.
#[test]
fn auto_dispatch_failure_marker_cleared_by_a_new_dispatch() {
    let mut app = make_app();
    app.update(auto_dispatch_failed_msg(1));

    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));

    assert!(!app.auto_dispatch_failed(TaskId(1)));
}

/// The other half of PersistsUntilRedispatched: a subtask that left backlog by
/// any route is no longer stalled, and the board learns that from the refreshed
/// row rather than from a message of its own.
#[test]
fn auto_dispatch_failure_marker_cleared_when_the_task_leaves_backlog() {
    let mut app = make_app();
    app.update(auto_dispatch_failed_msg(1));

    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        Box::new(make_task(1, TaskStatus::Running)),
    )));

    assert!(!app.auto_dispatch_failed(TaskId(1)));
}

/// A refreshed row that is still in backlog is still stalled — the update must
/// not clear the marker just because the row was reloaded (which it is, right
/// after the failure, by the chain's own TaskChanged event).
#[test]
fn auto_dispatch_failure_marker_survives_a_backlog_refresh() {
    let mut app = make_app();
    app.update(auto_dispatch_failed_msg(1));

    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        Box::new(make_task(1, TaskStatus::Backlog)),
    )));

    assert!(app.auto_dispatch_failed(TaskId(1)));
}

/// `@guarantee DistinctFromOrdinaryBacklog` in docs/specs/epics.allium — a
/// subtask released by a failed chain is otherwise byte-identical to one that
/// was never dispatched.
#[test]
fn stalled_chain_card_shows_auto_dispatch_failed() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.update(auto_dispatch_failed_msg(1));

    let buf = render_to_buffer(&mut app, 120, 20);

    assert!(
        buffer_contains(&buf, "\u{26a0} auto-dispatch failed"),
        "expected '⚠ auto-dispatch failed'"
    );
}

/// `@guarantee DispatchingOutranksIt`: retrying is the resolution, so the
/// retry's own spinner must not be masked by the failure it is resolving.
#[test]
fn stalled_chain_card_yields_to_the_dispatching_spinner() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.update(auto_dispatch_failed_msg(1));
    // Re-insert the marker behind the spinner: a real retry clears it, so this
    // asserts the render precedence rather than the clearing.
    app.mark_dispatching(TaskId(1));
    app.agents
        .auto_dispatch_failed
        .insert(TaskId(1), "stale".to_string());

    let buf = render_to_buffer(&mut app, 120, 20);

    assert!(
        buffer_contains(&buf, "dispatching"),
        "an in-flight dispatch must still show its spinner"
    );
    assert!(
        !buffer_contains(&buf, "auto-dispatch failed"),
        "the stale failure must not mask the retry"
    );
}
