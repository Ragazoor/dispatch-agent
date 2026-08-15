#![allow(clippy::unwrap_used, clippy::expect_used)]
use ratatui::buffer::Buffer;

use super::super::App;
use super::{make_app, make_epic_with_title, make_key, make_task, render_to_buffer};
use crate::models::{TaskId, TaskStatus};
use crossterm::event::KeyCode;

fn buffer_to_string(buf: &Buffer) -> String {
    let area = buf.area();
    let mut lines = Vec::with_capacity(area.height as usize);
    for y in area.top()..area.bottom() {
        let mut line = String::with_capacity(area.width as usize * 3);
        for x in area.left()..area.right() {
            line.push_str(buf[(x, y)].symbol());
        }
        line.truncate(line.trim_end().len());
        lines.push(line);
    }
    lines.join("\n")
}

fn render_to_string(app: &mut App, width: u16, height: u16) -> String {
    buffer_to_string(&render_to_buffer(app, width, height))
}

#[test]
fn snapshot_empty_kanban_board() {
    let mut app = App::new(vec![]);
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_kanban_with_tasks() {
    let mut app = make_app();
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_help_overlay() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('?')));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_input_title_form() {
    use super::super::types::{InputMode, TaskDraft};
    let mut app = make_app();
    app.input.mode = InputMode::InputTitle;
    app.input.set_buffer("My new task".to_string());
    app.input.task_draft = Some(TaskDraft::default());
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_input_repo_path_form() {
    use super::super::types::{InputMode, TaskDraft};
    use crate::models::TaskTag;
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo/alpha".to_string(), "/repo/beta".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.set_buffer(String::new());
    app.input.task_draft = Some(TaskDraft {
        title: "My new task".to_string(),
        description: "A description".to_string(),
        tag: Some(TaskTag::Feature),
        ..TaskDraft::default()
    });
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_quick_dispatch_form() {
    use super::super::types::InputMode;
    let mut app = make_app();
    app.board.repo_paths = vec!["/repo/alpha".to_string(), "/repo/beta".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_quick_dispatch_new_entry() {
    // Renders the picker with a non-empty buffer that fuzzy-matches an existing
    // repo and also shows the "+ new path" entry at the bottom.
    use super::super::types::InputMode;
    let mut app = make_app();
    app.board.repo_paths = vec!["/home/code/project-work".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    app.input.set_buffer("/home/code/work".to_string()); // fuzzy-matches existing, new entry shown
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_input_repo_path_form_with_new_entry() {
    // InputRepoPath now shows a "+ new path" entry when the buffer doesn't
    // exactly match any existing path — same contract as QuickDispatch.
    use super::super::types::{InputMode, TaskDraft};
    use crate::models::TaskTag;
    let mut app = make_app();
    app.board.repo_paths = vec!["/home/code/project-work".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.set_buffer("/home/code/work".to_string()); // fuzzy-matches existing, new entry shown
    app.input.task_draft = Some(TaskDraft {
        title: "My new task".to_string(),
        description: "A description".to_string(),
        tag: Some(TaskTag::Feature),
        ..TaskDraft::default()
    });
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_input_base_branch_form_with_history() {
    // BaseBranchPicker (task #3422): the base-branch field of the manual
    // "new task" form renders the current repo's remembered branch history
    // as a filtered list, mirroring InputRepoPath's RepoPathPicker.
    use super::super::types::{InputMode, TaskDraft};
    use crate::models::TaskTag;
    let mut app = make_app();
    app.board.repo_base_branches = std::collections::HashMap::from([(
        "/repo/alpha".to_string(),
        vec![
            "develop".to_string(),
            "main".to_string(),
            "release".to_string(),
        ],
    )]);
    app.input.mode = InputMode::InputBaseBranch;
    app.input.set_buffer("develop".to_string()); // PrefillFromHistory: most-recently-used
    app.input.task_draft = Some(TaskDraft {
        title: "My new task".to_string(),
        description: "A description".to_string(),
        tag: Some(TaskTag::Feature),
        repo_path: "/repo/alpha".to_string(),
        ..TaskDraft::default()
    });
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_confirm_retry_form() {
    use super::super::types::InputMode;
    use crate::models::TaskId;
    let mut app = make_app();
    app.input.mode = InputMode::ConfirmRetry(TaskId(1));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Baseline regression coverage for second-line card badges across the
/// most common variants (tags, sub-statuses, PR labels). Locks the layout
/// before introducing label rendering so any unintended shift in spacing
/// or styling is caught by the snapshot diff.
#[test]
fn snapshot_card_badges_baseline() {
    use super::super::App;
    use crate::models::{SubStatus, TaskStatus, TaskTag};

    let mut tasks = Vec::new();
    let mk = |id: i64, status: TaskStatus, title: &str| {
        let mut t = make_task(id, status);
        t.title = title.to_string();
        t
    };

    // Backlog: tag pills
    let mut t = mk(1, TaskStatus::Backlog, "bug task");
    t.tag = Some(TaskTag::Bug);
    tasks.push(t);
    let mut t = mk(2, TaskStatus::Backlog, "feature task");
    t.tag = Some(TaskTag::Feature);
    tasks.push(t);
    let mut t = mk(3, TaskStatus::Backlog, "chore task");
    t.tag = Some(TaskTag::Chore);
    tasks.push(t);
    let mut t = mk(4, TaskStatus::Backlog, "fix task");
    t.tag = Some(TaskTag::Fix);
    tasks.push(t);

    // Running: sub-status badges
    let mut t = mk(5, TaskStatus::Running, "active");
    t.sub_status = SubStatus::Active;
    t.worktree = Some("/wt".to_string());
    t.tmux_window = Some("w".to_string());
    tasks.push(t);
    let mut t = mk(6, TaskStatus::Running, "stale");
    t.sub_status = SubStatus::Stale;
    t.worktree = Some("/wt".to_string());
    t.tmux_window = Some("w".to_string());
    // Pin the known-timestamp branch of the stale card renderer.
    t.last_pre_tool_use_at = Some(chrono::Utc::now() - chrono::Duration::minutes(12));
    tasks.push(t);
    let mut t = mk(7, TaskStatus::Running, "needs input");
    t.sub_status = SubStatus::NeedsInput;
    t.worktree = Some("/wt".to_string());
    t.tmux_window = Some("w".to_string());
    tasks.push(t);
    let mut t = mk(8, TaskStatus::Running, "crashed");
    t.sub_status = SubStatus::Crashed;
    t.worktree = Some("/wt".to_string());
    // No window: detached out-prioritises crashed, which is what this row locks.
    t.tmux_window = None;
    tasks.push(t);

    // Review: PR labels + sub-statuses
    let mut t = mk(9, TaskStatus::Review, "awaiting review");
    t.sub_status = SubStatus::AwaitingReview;
    t.url = Some(crate::models::TaskUrl::new(
        "https://github.com/o/r/pull/42",
        crate::models::UrlType::Pr,
    ));
    tasks.push(t);
    let mut t = mk(10, TaskStatus::Review, "changes requested");
    t.sub_status = SubStatus::ChangesRequested;
    t.url = Some(crate::models::TaskUrl::new(
        "https://github.com/o/r/pull/43",
        crate::models::UrlType::Pr,
    ));
    tasks.push(t);
    let mut t = mk(11, TaskStatus::Review, "approved");
    t.sub_status = SubStatus::Approved;
    t.url = Some(crate::models::TaskUrl::new(
        "https://github.com/o/r/pull/44",
        crate::models::UrlType::Pr,
    ));
    tasks.push(t);

    // Done: merged PR
    let mut t = mk(12, TaskStatus::Done, "merged");
    t.url = Some(crate::models::TaskUrl::new(
        "https://github.com/o/r/pull/45",
        crate::models::UrlType::Pr,
    ));
    tasks.push(t);

    let mut app = App::new(tasks);
    app.spinner_tick = 0;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Render a card with labels alongside an existing PR badge to verify labels
/// compose with derived indicators on the second line without breaking
/// layout. Two cards: one under the cursor (highlighted background) and one
/// not, so the cursor-style interaction with label colours is also locked.
#[test]
fn snapshot_card_with_labels() {
    use super::super::App;
    use crate::models::TaskStatus;

    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.title = "CVE-2024-9999".to_string();
    t1.labels = vec!["scala-common".to_string(), "security".to_string()];

    let mut t2 = make_task(2, TaskStatus::Review);
    t2.title = "PR review".to_string();
    t2.labels = vec!["app-frontend".to_string()];
    t2.url = Some(crate::models::TaskUrl::new(
        "https://github.com/o/r/pull/77",
        crate::models::UrlType::Pr,
    ));

    let mut app = App::new(vec![t1, t2]);
    app.spinner_tick = 0;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_card_dispatching_indicator() {
    use super::super::types::Message;
    use crate::models::TaskId;

    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    // Pin the spinner frame so the rendered glyph is deterministic.
    app.spinner_tick = 0;

    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Locks the live-subagent-count suffix on a running card's second line.
#[test]
fn snapshot_card_running_with_subagents() {
    let mut t = make_task(1, TaskStatus::Running);
    t.title = "running with subagents".to_string();
    t.live_subagents = 3;

    let mut app = App::new(vec![t]);
    app.spinner_tick = 0;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Locks the live-shell-count suffix on a running card's second line.
#[test]
fn snapshot_card_running_with_shells() {
    let mut t = make_task(1, TaskStatus::Running);
    t.title = "running with shells".to_string();
    t.live_shells = 2;

    let mut app = App::new(vec![t]);
    app.spinner_tick = 0;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Locks composed subagent + shell suffixes.
#[test]
fn snapshot_card_running_with_subagents_and_shells() {
    let mut t = make_task(1, TaskStatus::Running);
    t.title = "running with both".to_string();
    t.live_subagents = 1;
    t.live_shells = 1;

    let mut app = App::new(vec![t]);
    app.spinner_tick = 0;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Locks the shell-stale card indicator.
#[test]
fn snapshot_card_stale_shell() {
    let mut t = make_task(1, TaskStatus::Running);
    t.title = "abandoned shell".to_string();
    t.sub_status = crate::models::SubStatus::StaleShell;
    t.live_shells = 1;
    t.oldest_live_shell_started_at = Some(chrono::Utc::now() - chrono::Duration::hours(5));

    let mut app = App::new(vec![t]);
    app.spinner_tick = 0;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_input_epic_title_form() {
    use super::super::types::{EpicDraft, InputMode};
    let mut app = make_app();
    app.input.mode = InputMode::InputEpicTitle;
    app.input.set_buffer("My new epic".to_string());
    app.input.epic_draft = Some(EpicDraft::default());
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

fn make_feed_epic(id: i64, title: &str, sort_order: i64) -> crate::models::Epic {
    let now = chrono::Utc::now();
    crate::models::Epic {
        id: crate::models::EpicId(id),
        title: title.to_string(),
        description: String::new(),
        status: crate::models::TaskStatus::Backlog,
        plan_path: None,
        sort_order: Some(sort_order),
        auto_dispatch: false,
        parent_epic_id: None,
        feed_command: Some(format!("feed-{title}")),
        feed_interval_secs: Some(30),
        created_at: now,
        updated_at: now,
        group_by_repo: false,
        feed_role: crate::models::FeedRole::None,
        origin: crate::models::EpicOrigin::Manual,
    }
}

#[test]
fn snapshot_top_indicators_in_board_mode() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_feed_epic(1, "My Feed", -2),
        make_feed_epic(2, "Another Feed", -1),
    ];
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_top_indicators_in_feed_epic_mode() {
    use super::super::types::Message;
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_feed_epic(1, "My Feed", -2),
        make_feed_epic(2, "Another Feed", -1),
    ];
    let feed_epic_id = app
        .epics()
        .iter()
        .find(|e| e.feed_command.is_some())
        .unwrap()
        .id;
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        feed_epic_id,
    )));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_group_indicator_on_non_feed_epic() {
    use super::super::types::Message;
    use super::make_epic;
    let mut app = App::new(vec![]);
    let mut epic = make_epic(1);
    epic.title = "My Feature".to_string();
    epic.group_by_repo = true;
    let epic_id = epic.id;
    app.board.epics = vec![epic];
    app.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(
        epic_id,
    )));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_kanban_with_archive_focused() {
    use super::super::types::Message;
    use super::make_app_with_archived_task;
    let mut app = make_app_with_archived_task();
    // Navigate to Archive (col 5 = COLUMN_COUNT + 1) — make_app starts at col 1 (Backlog)
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    assert_eq!(
        app.selected_column(),
        crate::models::TaskStatus::COLUMN_COUNT + 1
    );
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_task_detail_overlay_peek() {
    use crate::tui::Message;
    let mut app = App::new(vec![]);
    let mut task = make_task(1, TaskStatus::Backlog);
    task.description = "First line of description.\nSecond line.\nThird line.".to_string();
    task.repo_path = "/repo/my-project".to_string();
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    app.board.tasks.push(task);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::OpenDetail(TaskId(1)),
    ));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_task_detail_overlay_zoomed() {
    use crate::tui::{Message, ViewMode};
    let mut app = App::new(vec![]);
    let mut task = make_task(1, TaskStatus::Backlog);
    task.description = "First line of description.\nSecond line.\nThird line.".to_string();
    task.repo_path = "/repo/my-project".to_string();
    app.board.tasks.push(task);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::OpenDetail(TaskId(1)),
    ));
    if let ViewMode::TaskDetail { ref mut zoomed, .. } = app.board.view_mode {
        *zoomed = true;
    }
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_task_detail_overlay_empty_optional_fields() {
    use crate::tui::Message;
    let mut app = App::new(vec![]);
    let mut task = make_task(1, TaskStatus::Backlog);
    task.description = "Just a description.".to_string();
    task.repo_path = "/repo/path".to_string();
    // pr_url, plan_path, epic_id all None (default from make_task)
    app.board.tasks.push(task);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::OpenDetail(TaskId(1)),
    ));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn flat_view_epic_headers() {
    use crate::models::EpicId;
    use crate::tui::tests::make_epic_with_title;

    let mut app = App::new(vec![]);
    let epic = make_epic_with_title(10, "My Feature");
    app.board.epics = vec![epic];
    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(EpicId(10));
    t1.sort_order = Some(10);
    let mut t2 = make_task(2, TaskStatus::Running);
    t2.epic_id = Some(EpicId(10));
    t2.sort_order = Some(20);
    app.board.tasks = vec![t1, t2];
    app.board.flattened = true;
    app.selection_mut().set_column(2); // Running column

    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn flat_view_nested_epic_header() {
    use crate::models::EpicId;
    use crate::tui::tests::make_epic_with_title;

    let mut app = App::new(vec![]);
    // root(10) "PR Reviews" -> child(20) "Bots PR"
    let root = make_epic_with_title(10, "PR Reviews");
    let mut child = make_epic_with_title(20, "Bots PR");
    child.parent_epic_id = Some(EpicId(10));
    app.board.epics = vec![root, child];

    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(EpicId(20));
    t1.sort_order = Some(10);
    app.board.tasks = vec![t1];
    app.board.flattened = true;
    app.selection_mut().set_column(2); // Running column

    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn flat_view_orphan_separator() {
    use crate::models::EpicId;
    use crate::tui::tests::make_epic_with_title;

    let mut app = App::new(vec![]);
    let epic = make_epic_with_title(10, "My Feature");
    app.board.epics = vec![epic];
    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(EpicId(10));
    let mut t2 = make_task(2, TaskStatus::Running);
    t2.epic_id = None; // orphan — should trigger OrphanSeparator
    app.board.tasks = vec![t1, t2];
    app.board.flattened = true;
    app.selection_mut().set_column(2); // Running column

    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Snapshot: backlog column in flat mode still shows epic cards (backlog excluded from flattening).
#[test]
fn flat_view_backlog_shows_epic_card() {
    use crate::models::EpicId;
    use crate::tui::tests::make_epic_with_title;

    let mut app = App::new(vec![]);
    let epic = make_epic_with_title(10, "My Feature");
    app.board.epics = vec![epic];
    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.epic_id = Some(EpicId(10));
    app.board.tasks = vec![t1];
    app.board.flattened = true;
    app.selection_mut().set_column(1); // Backlog column

    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn flat_view_substatus_indicators_above_epic_headers() {
    use crate::models::{EpicId, SubStatus};
    use crate::tui::tests::make_epic_with_title;

    let mut app = App::new(vec![]);

    let epic_a = make_epic_with_title(10, "Epic Alpha");
    let epic_b = make_epic_with_title(20, "Epic Beta");
    app.board.epics = vec![epic_a, epic_b];

    // Running column: NeedsInput (priority 3) and Active (priority 5) groups.
    // Expected column order:
    //   ──── needs input ────
    //   ── Epic Alpha ──────
    //   Task 1
    //   ──── active ─────────
    //   ── Epic Beta ───────
    //   Task 2
    //   ── Epic Alpha ──────   ← Epic Alpha appears again under "active"
    //   Task 3
    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(EpicId(10));
    t1.sub_status = SubStatus::NeedsInput;
    t1.sort_order = Some(10);

    let mut t2 = make_task(2, TaskStatus::Running);
    t2.epic_id = Some(EpicId(20));
    t2.sub_status = SubStatus::Active;
    t2.sort_order = Some(20);

    let mut t3 = make_task(3, TaskStatus::Running);
    t3.epic_id = Some(EpicId(10));
    t3.sub_status = SubStatus::Active;
    t3.sort_order = Some(30);

    app.board.tasks = vec![t1, t2, t3];
    app.board.flattened = true;
    app.selection_mut().set_column(2); // Running = nav col 2

    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Snapshot: ▼ appears at the bottom of the Backlog column when tasks overflow the visible area.
/// Uses height=20 so the kanban area is short (≈8 rows), and 5 tasks × 3 rows = 15 > 8.
#[test]
fn snapshot_scroll_indicator_down() {
    let tasks: Vec<_> = (1..=5).map(|i| make_task(i, TaskStatus::Backlog)).collect();
    let mut app = App::new(tasks);
    // Cursor at top: offset=0, only ▼ shows.
    app.selection_mut().set_row(1, 0);
    let rendered = render_to_string(&mut app, 120, 20);
    insta::assert_snapshot!(rendered);
}

/// Snapshot: ▲ appears at the top border of the Backlog column when scrolled past items above.
/// Cursor on the last task forces ratatui to scroll, making offset > 0.
#[test]
fn snapshot_scroll_indicator_up() {
    let tasks: Vec<_> = (1..=5).map(|i| make_task(i, TaskStatus::Backlog)).collect();
    let mut app = App::new(tasks);
    // Cursor on task 5 (row index 4): ratatui adjusts offset to show it → ▲ appears.
    app.selection_mut().set_row(1, 4);
    let rendered = render_to_string(&mut app, 120, 20);
    insta::assert_snapshot!(rendered);
}

#[test]
fn reparent_epic_overlay_renders() {
    use super::make_epic;
    use crate::models::EpicId;
    let mut app = make_app();
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.handle_start_reparent(EpicId(10));

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    terminal
        .draw(|f| crate::tui::ui::render(f, &mut app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    insta::assert_snapshot!(format!("{:#?}", buffer));
}

#[test]
fn move_task_to_epic_overlay_renders() {
    use super::make_epic;
    use crate::models::TaskId;
    let mut app = make_app();
    app.board.epics = vec![make_epic(10), make_epic(20)];
    app.handle_start_move_to_epic(TaskId(1));

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    terminal
        .draw(|f| crate::tui::ui::render(f, &mut app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    insta::assert_snapshot!(format!("{:#?}", buffer));
}

#[test]
fn snapshot_board_with_active_search() {
    let mk = |id: i64, title: &str| {
        let mut t = make_task(id, TaskStatus::Backlog);
        t.title = title.to_string();
        t
    };
    let mut app = App::new(vec![
        mk(1, "Fix login bug"),
        mk(2, "Add search feature"),
        mk(3, "Refactor parser"),
    ]);
    app.search.query = "search".to_string();
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_todo_list_with_done_items() {
    use crate::models::{Todo, TodoId};
    use crate::tui::messages::TodoMessage;
    use chrono::Utc;

    let mk = |id: i64, title: &str, done: bool, so: i64| Todo {
        id: TodoId(id),
        title: title.into(),
        done,
        sort_order: so,
        parent_id: None,
        linked: None,
        created_at: Utc::now(),
    };

    let mut app = App::new(vec![]);
    app.update(crate::tui::Message::Todo(TodoMessage::Show(vec![
        mk(1, "Reply to Sven re: scheduler", false, 0),
        mk(2, "Prep standup notes", false, 1),
        mk(3, "Merge planner spec", true, 2),
    ])));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn todos_overlay_shows_task_and_epic_badges() {
    use crate::models::{EpicId, TaskId, TodoId, TodoLink};
    use crate::tui::messages::TodoMessage;

    let mut app = App::new(vec![]);
    let todos = vec![
        {
            crate::models::Todo {
                id: TodoId(1),
                title: "Linked to task".to_string(),
                done: false,
                sort_order: 0,
                parent_id: None,
                linked: Some(TodoLink::Task(TaskId(42))),
                created_at: chrono::Utc::now(),
            }
        },
        {
            crate::models::Todo {
                id: TodoId(2),
                title: "Linked to epic".to_string(),
                done: false,
                sort_order: 1,
                parent_id: None,
                linked: Some(TodoLink::Epic(EpicId(7))),
                created_at: chrono::Utc::now(),
            }
        },
        {
            crate::models::Todo {
                id: TodoId(3),
                title: "Unlinked todo".to_string(),
                done: false,
                sort_order: 2,
                parent_id: None,
                linked: None,
                created_at: chrono::Utc::now(),
            }
        },
    ];
    app.update(crate::tui::Message::Todo(TodoMessage::Show(todos)));

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    terminal
        .draw(|f| crate::tui::ui::render(f, &mut app))
        .unwrap();
    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn snapshot_board_in_search_input_mode() {
    use crate::tui::InputMode;
    let mk = |id: i64, title: &str| {
        let mut t = make_task(id, TaskStatus::Backlog);
        t.title = title.to_string();
        t
    };
    let mut app = App::new(vec![
        mk(1, "Fix login bug"),
        mk(2, "Add search feature"),
        mk(3, "Refactor parser"),
    ]);
    app.search.query = "search".to_string();
    // While in SearchTasks input mode the status bar shows the live search
    // prompt: "Search board: {query}_   [Enter] keep  [Esc] cancel".
    app.input.mode = InputMode::SearchTasks;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_reparent_epic_popup() {
    use crate::models::EpicId;
    use crate::tui::messages::EpicMessage;
    use crate::tui::Message;

    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![
        make_epic_with_title(10, "Platform"),
        make_epic_with_title(20, "Backend"),
        make_epic_with_title(30, "Frontend"),
    ];
    // Use the opener handler so tree state is properly initialized.
    app.update(Message::Epic(EpicMessage::StartReparent(EpicId(10))));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_reparent_epic_popup_after_nav() {
    use crate::models::EpicId;
    use crate::tui::messages::EpicMessage;
    use crate::tui::types::TreeNav;
    use crate::tui::Message;

    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![
        make_epic_with_title(10, "Platform"),
        make_epic_with_title(20, "Backend"),
        make_epic_with_title(30, "Frontend"),
    ];
    app.update(Message::Epic(EpicMessage::StartReparent(EpicId(10))));
    // Navigate down once so the cursor is on the first epic entry.
    app.update(Message::Epic(EpicMessage::ReparentNavigate(TreeNav::Down)));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_move_task_to_epic_popup() {
    use crate::models::{TaskId, TaskStatus};
    use crate::tui::messages::TaskMessage;
    use crate::tui::Message;

    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![
        make_epic_with_title(10, "Platform"),
        make_epic_with_title(20, "Backend"),
    ];
    // Use the opener handler so tree state is properly initialized.
    app.update(Message::Task(TaskMessage::StartMoveToEpic(TaskId(1))));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_repo_filter_popup() {
    use crate::tui::Message;

    let mut app = make_app();
    app.board.repo_paths = vec![
        "/home/user/projects/alpha".to_string(),
        "/home/user/projects/beta".to_string(),
        "/home/user/projects/gamma".to_string(),
    ];
    app.update(Message::RepoFilter(
        crate::tui::messages::RepoFilterMessage::Start,
    ));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_repo_filter_popup_cursor_on_repo() {
    use crate::tui::messages::RepoFilterMessage;
    use crate::tui::Message;

    let mut app = make_app();
    app.board.repo_paths = vec![
        "/home/user/projects/alpha".to_string(),
        "/home/user/projects/beta".to_string(),
        "/home/user/projects/gamma".to_string(),
    ];
    app.update(Message::RepoFilter(RepoFilterMessage::Start));
    // Move cursor down once to position it on the first repo row.
    app.update(Message::RepoFilter(RepoFilterMessage::MoveCursor(1)));
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_top_row_budget_indicator_fresh() {
    use crate::models::budget::{BudgetSnapshot, BudgetWindow};

    // render_top_indicators reads the real wall clock for `now`, so a fresh
    // snapshot must be captured close to it, not at a fixed epoch. `now` here
    // and the `now` read inside render_top_indicators can differ by up to a
    // second or two (two separate Utc::now().timestamp() calls, and the
    // render's call always happens at or after this one — never before).
    // Offsets below are chosen mid-bucket (e.g. 8070s sits in the middle of
    // the "2h14m" second-range, not at its 8040s boundary with "2h13m") so
    // that skew can never flip the rendered digit and flake the snapshot.
    let now = chrono::Utc::now().timestamp();
    let mut app = make_app();
    app.budget = Some(BudgetSnapshot {
        five_hour: Some(BudgetWindow {
            used_percentage: 23.4,
            resets_at: now + 8070,
        }),
        seven_day: Some(BudgetWindow {
            used_percentage: 41.2,
            resets_at: now + 349_200,
        }),
        captured_at: now,
    });
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_top_row_budget_indicator_stale() {
    use crate::models::budget::{BudgetSnapshot, BudgetWindow};

    // captured_at far enough in the past (relative to render_top_indicators'
    // real wall-clock "now") to exceed BUDGET_STALE_AFTER (600s). 1_020s (17m)
    // is mid-bucket for the "17m old" age label too: the label only flips at
    // a 60s boundary (1_020..1_079 all render "17m"), and skew between this
    // `now` and render_top_indicators' own `Utc::now()` call only ever adds a
    // second or two, never subtracts — so it cannot cross either boundary.
    let now = chrono::Utc::now().timestamp();
    let captured_at = now - 1_020;
    let mut app = make_app();
    app.budget = Some(BudgetSnapshot {
        five_hour: Some(BudgetWindow {
            used_percentage: 91.0,
            resets_at: captured_at + 8070,
        }),
        seven_day: Some(BudgetWindow {
            used_percentage: 41.2,
            // Not 349_200: this offset is added to `captured_at` (already
            // 1_020s in the past), so its effective countdown is 1_020s
            // smaller than the fresh test's. 345_600 here lands mid-bucket
            // at 344_580s ("3d", with ~1_019s of margin to the next
            // boundary) — safe as-is; only the fresh test's 345_600 sat
            // exactly on a boundary.
            resets_at: captured_at + 345_600,
        }),
        captured_at,
    });
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_board_search_narrows_epic_cards() {
    use super::make_epic_with_title;
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "login".to_string();
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}
