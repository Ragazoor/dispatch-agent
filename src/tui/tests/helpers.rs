#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{Epic, EpicId, SubStatus, TaskId, TaskStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier},
    Terminal,
};

/// Check whether a rendered buffer contains the given text anywhere.
pub(in crate::tui) fn buffer_contains(buf: &Buffer, text: &str) -> bool {
    let area = buf.area();
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        for x in area.left()..area.right() {
            line.push_str(buf[(x, y)].symbol());
        }
        if line.contains(text) {
            return true;
        }
    }
    false
}

/// Helper: render the app into a test terminal and return the buffer.
pub(in crate::tui) fn render_to_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

pub(in crate::tui) fn make_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Strip telemetry `RecordUsageEvent` entries from a `cmds` vec. Used by tests
/// that assert exact command counts and don't care about usage telemetry.
pub(in crate::tui) fn without_usage(cmds: Vec<Command>) -> Vec<Command> {
    cmds.into_iter()
        .filter(|c| !matches!(c, Command::RecordUsageEvent(_)))
        .collect()
}

/// A lone `g` press only starts the pending `gg`-chord window (see
/// [`crate::tui::PendingAction::GChord`]); this backdates it past
/// `GG_CHORD_TIMEOUT` and ticks to simulate the user going idle, so tests can
/// assert the idle backstop clears the stale chord (with no action firing)
/// without a real sleep.
pub(in crate::tui) fn resolve_pending_g_via_idle_tick(app: &mut App) -> Vec<Command> {
    app.interaction.pending = crate::tui::PendingAction::GChord(
        std::time::Instant::now()
            - crate::tui::GG_CHORD_TIMEOUT
            - std::time::Duration::from_millis(50),
    );
    app.handle_tick()
}

/// A task fixture. Running/Review tasks come provisioned — a dispatched task
/// has a worktree and a window, and leaving them null would make the default
/// fixture `is_unprovisioned` (rendering "⚠ no worktree" on every card). Tests
/// that want an unprovisioned or detached task clear the fields explicitly.
pub(in crate::tui) fn make_task(id: i64, status: TaskStatus) -> Task {
    let now = chrono::Utc::now();
    let provisioned = matches!(status, TaskStatus::Running | TaskStatus::Review);
    Task {
        id: TaskId(id),
        title: format!("Task {id}"),
        description: String::new(),
        repo_path: String::from("/repo"),
        status,
        worktree: provisioned.then(|| format!("/repo/.worktrees/{id}-task-{id}")),
        tmux_window: provisioned.then(|| format!("task-{id}")),
        plan_path: None,
        epic_id: None,
        sub_status: SubStatus::default_for(status),
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
    }
}

/// A Running/Review task with neither a worktree nor a window — the state
/// `UnprovisionedIndicator` in `docs/specs/dispatch.allium` covers. Also
/// carries no activity stamp, so `App::dispatch_may_be_in_flight` reads it as
/// a dead claim rather than one still provisioning.
pub(in crate::tui) fn make_unprovisioned_task(id: i64, status: TaskStatus) -> Task {
    Task {
        worktree: None,
        tmux_window: None,
        last_pre_tool_use_at: None,
        ..make_task(id, status)
    }
}

pub(in crate::tui) fn make_app() -> App {
    App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
        make_task(3, TaskStatus::Running),
        make_task(4, TaskStatus::Done),
    ])
}

pub(in crate::tui) fn make_shift_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

/// Extract bold key spans (like "[d]", "[Tab]") from hint spans.
pub(in crate::tui) fn hint_keys<'a>(hints: &'a [ratatui::text::Span<'static>]) -> Vec<&'a str> {
    hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect()
}

pub(in crate::tui) fn make_epic(id: i64) -> Epic {
    let now = chrono::Utc::now();
    Epic {
        id: EpicId(id),
        title: format!("Epic {id}"),
        description: String::new(),
        status: TaskStatus::Backlog,
        plan_path: None,
        sort_order: None,
        auto_dispatch: false,
        parent_epic_id: None,
        feed_command: None,
        feed_interval_secs: None,
        group_by_repo: false,
        feed_role: crate::models::FeedRole::None,
        origin: crate::models::EpicOrigin::Manual,
        created_at: now,
        updated_at: now,
    }
}

pub(in crate::tui) fn make_epic_with_title(id: i64, title: &str) -> Epic {
    Epic {
        title: title.to_string(),
        ..make_epic(id)
    }
}

pub(in crate::tui) fn make_todo(id: i64, title: &str) -> crate::models::Todo {
    crate::models::Todo {
        id: crate::models::TodoId(id),
        title: title.into(),
        done: false,
        sort_order: id,
        parent_id: None,
        linked: None,
        created_at: chrono::Utc::now(),
    }
}

pub(in crate::tui) fn make_app_with_archived_task() -> App {
    let mut app = make_app();
    let mut t = make_task(10, TaskStatus::Archived);
    t.title = "archived task".to_string();
    app.board.tasks.push(t);
    app
}

/// Helper: create an app with one task + one epic in Backlog, cursor on the epic.
pub(in crate::tui) fn make_app_with_epic_selected() -> App {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    // Same priority (5), task (id=1) at row 0, epic (id=10) at row 1
    app.selection_mut().set_column(1); // Backlog = nav col 1
    app.selection_mut().set_row(1, 1);
    app
}

pub(in crate::tui) fn make_app_confirm_archive_epic() -> App {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    app.selection_mut().set_column(1); // Backlog = nav col 1
    app.selection_mut().set_row(1, 1); // cursor on epic (same priority as task, sorts after by id)
    app.input.mode = InputMode::ConfirmArchiveEpic;
    app.status.message = Some("Archive epic and all subtasks? [y/n]".to_string());
    app
}

/// Helper: create a `ReparentPickerState` for `epic_id` with a default tree state.
pub(in crate::tui) fn make_reparent_picker(epic_id: EpicId) -> crate::tui::ReparentPickerState {
    crate::tui::ReparentPickerState {
        epic_id,
        tree_state: std::cell::RefCell::new(tui_tree_widget::TreeState::default()),
        items: vec![],
    }
}

pub(in crate::tui) fn make_review_subtask(id: i64, epic_id: i64, sort_order: i64) -> Task {
    let mut task = make_task(id, TaskStatus::Review);
    task.epic_id = Some(EpicId(epic_id));
    task.worktree = Some(format!("/repo/.worktrees/{id}-task-{id}"));
    task.sort_order = Some(sort_order);
    task
}

/// Find a text span in the buffer and return the style of its first character.
pub(in crate::tui) fn find_style_of(buf: &Buffer, text: &str) -> Option<ratatui::style::Style> {
    let area = buf.area();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let remaining = (area.right() - x) as usize;
            if remaining < text.len() {
                break;
            }
            let segment: String = (0..text.len() as u16)
                .map(|dx| buf[(x + dx, y)].symbol().to_string())
                .collect();
            if segment == text {
                return Some(buf[(x, y)].style());
            }
        }
    }
    None
}

/// Extract the foreground color of the first `[` bracket in the given row.
pub(in crate::tui) fn first_bracket_fg(buf: &Buffer, row: u16) -> Option<Color> {
    let area = buf.area();
    for x in area.left()..area.right() {
        if buf[(x, row)].symbol() == "[" {
            return Some(buf[(x, row)].fg);
        }
    }
    None
}

pub(in crate::tui) fn app_with_tips() -> App {
    let mut app = App::new(vec![]);
    app.update(Message::Tips(crate::tui::messages::TipsMessage::Show {
        tips: make_tips(),
        starting_index: 1,
        max_seen_id: 0,
        show_mode: crate::models::TipsShowMode::Always,
    }));
    app
}

pub(in crate::tui) fn make_tips() -> Vec<crate::tips::Tip> {
    vec![
        crate::tips::Tip {
            id: 1,
            title: "Tip One".into(),
            body: "Body one".into(),
        },
        crate::tips::Tip {
            id: 2,
            title: "Tip Two".into(),
            body: "Body two".into(),
        },
        crate::tips::Tip {
            id: 3,
            title: "Tip Three".into(),
            body: "Body three".into(),
        },
    ]
}

pub(in crate::tui) fn make_tip_with_id(id: u32) -> crate::tips::Tip {
    crate::tips::Tip {
        id,
        title: format!("Tip {id}"),
        body: format!("Body {id}"),
    }
}

pub(in crate::tui) fn determine_tips_start(
    tips: &[crate::tips::Tip],
    seen_up_to: u32,
    show_mode: crate::models::TipsShowMode,
) -> Option<usize> {
    crate::runtime::tips_starting_index(tips, seen_up_to, show_mode)
}
