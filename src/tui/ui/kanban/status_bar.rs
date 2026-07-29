//! Status bar at the bottom of the kanban board.
//!
//! Renders one of three flavours depending on app state:
//! * a transient status message,
//! * archive-mode hints, or
//! * mode-specific hints (Normal mode delegates to `action_hints` /
//!   `epic_action_hints` / `batch_action_hints`).

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::super::palette::{CYAN, MUTED, YELLOW};
use super::super::shared::push_hint_spans;
use super::{action_hints, epic_action_hints};
use crate::tui::{App, ColumnItem, InputMode};

pub(super) fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let (line, style) = status_line(app, area);
    frame.render_widget(Paragraph::new(line).style(style), area);
}

/// Prepend `prefix` spans to `spans` in place, preserving order.
fn prepend(spans: &mut Vec<Span<'static>>, mut prefix: Vec<Span<'static>>) {
    prefix.append(spans);
    *spans = prefix;
}

/// A simple text status line with a single foreground colour, falling back to
/// `app.status.message` when set. (`app.status.message` being `Some` is already
/// short-circuited by `status_line`, so this always resolves to `default`; the
/// override arms retain this shape for clarity.)
fn hint_text(app: &App, default: &str, color: Color) -> (Line<'static>, Style) {
    let text = app.status.message.as_deref().unwrap_or(default).to_string();
    (Line::from(text), Style::default().fg(color))
}

/// A fixed text status line with a single foreground colour. Accepts anything
/// that converts into a `Line<'static>`, so string literals borrow without an
/// allocation while owned `String`s (e.g. a formatted search prompt) move in.
fn hint(text: impl Into<Line<'static>>, color: Color) -> (Line<'static>, Style) {
    (text.into(), Style::default().fg(color))
}

/// Build the passive main-session badge for the status bar, or `None` when it
/// should be hidden. Three states (docs/specs/dispatch.allium: MainSessionIndicator):
/// alive → `● main` (green); configured-but-not-alive → `○ main` (dim); neither
/// → hidden. Liveness is authoritative: an alive window shows the badge even if
/// no directory was ever configured.
fn main_session_badge(app: &App) -> Option<Vec<Span<'static>>> {
    let (glyph, color) = if app.main_session_alive {
        ("● main ", Color::Green)
    } else if app.main_session_dir().is_some() {
        ("○ main ", MUTED)
    } else {
        return None;
    };
    Some(vec![Span::styled(
        glyph,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )])
}

/// Compute the status bar content (a styled `Line` plus a base paragraph style)
/// for the current app state. Rendering happens once, in `render_status_bar`.
///
/// The two structurally-heavier flavours — archive-mode hints and the composed
/// Normal-mode hint line — live in dedicated builders (`archive_status_line`,
/// `normal_status_line`); everything else is a fixed per-mode hint.
fn status_line(app: &App, area: Rect) -> (Line<'static>, Style) {
    if let Some(msg) = &app.status.message {
        return (Line::from(msg.clone()), Style::default().fg(Color::Yellow));
    }

    // Archive mode status bar
    if app.show_archived() {
        return archive_status_line();
    }

    match &app.input.mode {
        InputMode::Normal => normal_status_line(app),
        InputMode::SearchTasks => hint(
            format!(
                "Search tasks: {}_   [Enter] keep  [Esc] cancel",
                app.search.query
            ),
            Color::Cyan,
        ),
        InputMode::InputTitle => hint("Creating task: enter title", Color::Yellow),
        InputMode::InputDescription => hint(
            "Creating task: opening $EDITOR for description",
            Color::Yellow,
        ),
        InputMode::InputRepoPath => hint("Creating task: enter repo path", Color::Yellow),
        InputMode::InputTag => hint_text(
            app,
            "Tag: [b]ug  [f]eature  [c]hore  [e]pic  [Enter] none",
            Color::Yellow,
        ),
        InputMode::ConfirmDelete => hint_text(app, "Delete? [y/n]", Color::Red),
        InputMode::QuickDispatch => hint("Quick dispatch: select repo path", Color::Yellow),
        InputMode::ConfirmRetry(_) => hint("[r] Resume  [f] Fresh start  [Esc] Cancel", Color::Red),
        InputMode::ConfirmArchive(_) => hint("Archive task? [y/n]", Color::Yellow),
        InputMode::ConfirmDone(_) => hint_text(app, "Move to Done? [y/n]", Color::Yellow),
        InputMode::InputEpicTitle => hint("Creating epic: enter title", Color::Magenta),
        InputMode::InputEpicDescription => hint(
            "Creating epic: opening $EDITOR for description",
            Color::Magenta,
        ),
        InputMode::ConfirmDeleteEpic => {
            hint_text(app, "Delete epic and subtasks? [y/n]", Color::Red)
        }
        InputMode::ConfirmArchiveEpic => hint("Archive epic and subtasks? [y/n]", Color::Yellow),
        InputMode::Help => hint("[?] or [Esc] to close help", Color::Cyan),
        InputMode::RepoFilter => hint(
            "Filter repos: [1-9] toggle  [a] all  [q/Esc] close",
            Color::Cyan,
        ),
        InputMode::ConfirmWrapUp(_) => hint_text(
            app,
            "Wrap up: [r] rebase  [p] create PR  [Esc] cancel",
            Color::Yellow,
        ),
        InputMode::InputPresetName => {
            hint("Enter preset name, [Enter] save, [Esc] cancel", Color::Cyan)
        }
        InputMode::ConfirmDeletePreset => hint("[A-Z] delete preset  [Esc] cancel", Color::Cyan),
        InputMode::ConfirmDeleteRepoPath => hint(
            "Delete repo path? y to confirm, any key to cancel",
            Color::Yellow,
        ),
        InputMode::ConfirmEpicWrapUp(_) => hint_text(
            app,
            "Epic wrap up: [r] rebase all  [p] PR all  [Esc] cancel",
            Color::Yellow,
        ),
        InputMode::ConfirmDetachTmux(_) => {
            hint_text(app, "Detach tmux panel? [y/n]", Color::Yellow)
        }
        InputMode::ConfirmQuit => hint("Quit dispatch? [y/n]", Color::Yellow),
        InputMode::InputBaseBranch => hint_text(app, "Base branch: ", Color::Yellow),
        InputMode::InputWrapUpMode => hint_text(
            app,
            "Wrap-up: [r]ebase  [p]r  [d]one  [Enter] skip",
            Color::Yellow,
        ),
        InputMode::MainSessionDir => {
            let line = crate::tui::ui::caret_field_line(
                area.width,
                "Main session directory:  ",
                "  [Enter] open  [Esc] cancel",
                &app.input.buffer,
                app.input.caret,
                Style::default().fg(Color::Cyan),
            );
            (line, Style::default())
        }
        InputMode::ReparentEpic(_) => hint(
            "Select new parent: navigate tree above, Enter to select",
            Color::Magenta,
        ),
        InputMode::ConfirmReparentEpic { .. } => {
            hint_text(app, "Reparent epic? [y/n]", Color::Magenta)
        }
        InputMode::MoveTaskToEpic(_) => hint(
            "Select target epic: navigate tree above, Enter to select",
            Color::Magenta,
        ),
        InputMode::ConfirmMoveTaskToEpic { .. } => {
            hint_text(app, "Move task to epic? [y/n]", Color::Magenta)
        }
        InputMode::ManagedFeedConfig => hint_text(
            app,
            "Managed feed config: Tab/arrows to move, Enter to save, Esc to cancel",
            Color::Cyan,
        ),
        InputMode::TodoTitle | InputMode::TodoQuickAdd => {
            let label = if matches!(app.input.mode, InputMode::TodoTitle) {
                "New todo"
            } else {
                "Quick add"
            };
            let line = crate::tui::ui::caret_field_line(
                area.width,
                &format!("{label}: "),
                "  [Enter] save  [Esc] cancel",
                &app.input.buffer,
                app.input.caret,
                Style::default().fg(Color::Yellow),
            );
            (line, Style::default())
        }
        InputMode::ConfirmDeleteTodo => hint("Delete todo? [y/n]", Color::Red),
        InputMode::LinkTodoToTask(_) => hint_text(
            app,
            "Navigate to a task or epic and press Enter to link — Esc to cancel",
            Color::Cyan,
        ),
        InputMode::ConfirmTrustRepo { .. } | InputMode::ConfirmTrustRepoQuickDispatch { .. } => {
            hint_text(app, "Repo not trusted — trust it? [y/N]", Color::Yellow)
        }
    }
}

/// Archive-mode status bar: a fixed row of `[key] label` hints.
fn archive_status_line() -> (Line<'static>, Style) {
    let key_color = MUTED;
    let label_style = Style::default().fg(MUTED);
    let key_style = Style::default().fg(key_color).add_modifier(Modifier::BOLD);
    let spans = vec![
        Span::styled("[x]", key_style),
        Span::styled(" delete  ", label_style),
        Span::styled("[e]", key_style),
        Span::styled(" edit  ", label_style),
        Span::styled("[H]", key_style),
        Span::styled(" close  ", label_style),
        Span::styled("[q]", key_style),
        Span::styled(" quit  ", label_style),
    ];
    (Line::from(spans), Style::default())
}

/// Normal-mode status bar: the base action hints (batch / epic / task) with the
/// active-mode badges (split, flat, active-filter, search, main-session) and the
/// open-todo count composed around them.
fn normal_status_line(app: &App) -> (Line<'static>, Style) {
    let key_color = CYAN;
    let mut spans = if app.has_selection() {
        let count = app.selected_tasks().len() + app.selected_epics().len();
        let has_tasks = !app.selected_tasks().is_empty();
        batch_action_hints(count, key_color, has_tasks)
    } else if let Some(ColumnItem::Epic(epic)) = app.selected_column_item() {
        epic_action_hints(epic, key_color)
    } else {
        action_hints(app.selected_task(), app.selected_column(), key_color)
    };
    if app.split_active() {
        prepend(
            &mut spans,
            vec![
                Span::styled(
                    "[S]",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("plit ", Style::default().fg(Color::Green)),
            ],
        );
    }
    if app.board.flattened {
        prepend(
            &mut spans,
            vec![Span::styled(
                "[flat] ",
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            )],
        );
    }
    if app.filter_only_active() {
        prepend(
            &mut spans,
            vec![Span::styled(
                "[active] ",
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            )],
        );
    }
    if app.search_active() {
        prepend(
            &mut spans,
            vec![Span::styled(
                format!("[/{}] ", app.search.query),
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            )],
        );
    }
    if let Some(badge) = main_session_badge(app) {
        prepend(&mut spans, badge);
    }
    if app.board.todo_open_count > 0 {
        spans.push(Span::styled(
            format!(" ({}) ", app.board.todo_open_count),
            Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
        ));
    }
    (Line::from(spans), Style::default())
}

/// Build status bar hints when tasks are batch-selected.
fn batch_action_hints(count: usize, key_color: Color, has_tasks: bool) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(MUTED);
    let count_style = Style::default().fg(YELLOW).add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(format!("{count} selected  "), count_style));

    let mut push_hint = |key: &'static str, label: &'static str| {
        push_hint_spans(&mut spans, key, label, key_color, label_style);
    };

    if has_tasks {
        push_hint("L", "move");
        push_hint("H", "back");
    }
    // 'x' completes tasks that aren't Done yet and archives the rest, so with
    // tasks selected the label can't commit to one verb. An epics-only
    // selection always archives.
    push_hint("x", if has_tasks { "done/archive" } else { "archive" });
    push_hint("a", "select all");
    push_hint("F", "flat");
    push_hint("v", "toggle");
    push_hint("Esc", "clear");
    spans
}
