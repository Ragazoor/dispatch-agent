//! Kanban board rendering: top-level entry point, summary/status bar, and
//! shared color helpers. Card, column, popup, and project-panel rendering
//! live in sibling sub-modules.

mod cards;
mod columns;
mod popups;
mod status_bar;

pub(in crate::tui) use popups::build_reparent_tree;

#[cfg(test)]
mod tests;

use super::input_form::{
    confirm_retry_lines, input_base_branch_lines, input_description_lines,
    input_epic_description_lines, input_epic_title_lines, input_repo_path_lines, input_tag_lines,
    input_title_lines, input_wrap_up_mode_lines, main_session_dir_lines, quick_dispatch_lines,
};
use super::learnings::render_learnings;
use super::palette::{ARCHIVE_STRIPE, BLUE, BORDER, CYAN, FG, GREEN, MUTED, PURPLE, YELLOW};
use super::shared::{push_hint_spans, render_top_indicators};
use super::todos::render_todos;

use crate::models::{Epic, Task, TaskStatus};
use crate::tui::{is_edge_column, App, ColumnItem, ColumnLayout, InputMode};
use chrono::Utc;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use columns::{compute_columns_data, render_columns};
use popups::{
    render_error_popup, render_help_overlay, render_managed_feed_config_overlay,
    render_move_task_overlay, render_reparent_epic_overlay, render_repo_filter_overlay,
    render_task_detail_overlay, render_tips_overlay,
};
use status_bar::render_status_bar;

/// Column color per status
pub(in crate::tui) fn column_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => BLUE,
        TaskStatus::Running => YELLOW,
        TaskStatus::Review => PURPLE,
        TaskStatus::Done => GREEN,
        TaskStatus::Archived => MUTED,
    }
}

/// Tinted background for the cursor card in each column.
pub(in crate::tui) fn cursor_bg_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => Color::Rgb(34, 38, 66),
        TaskStatus::Running => Color::Rgb(62, 50, 28),
        TaskStatus::Review => Color::Rgb(50, 34, 66),
        TaskStatus::Done => Color::Rgb(32, 52, 36),
        TaskStatus::Archived => Color::Rgb(34, 38, 66),
    }
}

/// Faint background wash for the focused column, tinted to the column color.
/// Must be just barely visible against the terminal bg (~26,27,38) so the
/// cursor card highlight (cursor_bg_color) stands out clearly on top of it.
pub(in crate::tui) fn column_bg_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => Color::Rgb(28, 30, 44),
        TaskStatus::Running => Color::Rgb(38, 34, 26),
        TaskStatus::Review => Color::Rgb(34, 28, 44),
        TaskStatus::Done => Color::Rgb(27, 36, 30),
        TaskStatus::Archived => Color::Rgb(28, 30, 44),
    }
}

/// Unicode status icon for the metadata line of each card.
pub(super) fn status_icon(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Backlog => "◦",
        TaskStatus::Running => "◉",
        TaskStatus::Review => "◎",
        TaskStatus::Done => "✓",
        TaskStatus::Archived => "◦",
    }
}

/// Compute how tall the detail/input panel should be based on the current input mode.
/// Expands when a repo list is being shown so all repos (plus cursor) are visible.
fn input_panel_height(app: &App, area_height: u16) -> u16 {
    // Fixed overhead: indicators(1) + summary(1) + kanban_min(6) + status_bar(1) = 9
    let overhead: u16 = 9;
    let max_height = area_height.saturating_sub(overhead).max(8);
    match &app.input.mode {
        InputMode::QuickDispatch => {
            // header(1) + blank(1) + filter(1) + repos(N) + new_entry(0|1) + blank(1) + hint(1) + borders(2)
            let filtered = crate::tui::filtered_repos(&app.board.repo_paths, &app.input.buffer);
            let new_entry = crate::tui::has_new_repo_option(&app.input.buffer, &filtered);
            let n = filtered.len() + new_entry as usize;
            let rows = n as u16 + 7;
            rows.clamp(8, max_height)
        }
        InputMode::MainSessionDir => {
            // header(1) + blank(1) + filter(1) + repos(N) + blank(1) + hint(1) + borders(2) = N + 7
            let n = app
                .board
                .repo_paths
                .iter()
                .filter(|p| crate::tui::fuzzy_matches(p, &app.input.buffer))
                .count();
            let rows = n as u16 + 7;
            rows.clamp(8, max_height)
        }
        InputMode::InputRepoPath if app.input.buffer.is_empty() => {
            // title(1) + desc(1) + path_input(1) + repos(N) + blank(1) + hint(1) + borders(2) = N + 7
            let rows = app.board.repo_paths.len() as u16 + 7;
            rows.clamp(8, max_height)
        }
        _ => 8,
    }
}

/// Top-level render function.
pub fn render(frame: &mut Frame, app: &mut App) {
    let full_area = frame.area();
    let now = Utc::now();

    // When split mode is active, wrap everything in a focus border.
    let area = if app.split_active() {
        let border_color = if app.split_focused() { CYAN } else { BORDER };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(border_color));
        frame.render_widget(block, full_area);
        Rect {
            x: full_area.x + 1,
            y: full_area.y + 1,
            width: full_area.width.saturating_sub(2),
            height: full_area.height.saturating_sub(2),
        }
    } else {
        full_area
    };

    let panel_h = input_panel_height(app, area.height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![
            Constraint::Length(1),       // top indicator bar
            Constraint::Length(1),       // summary row
            Constraint::Min(6),          // kanban board
            Constraint::Length(panel_h), // input form
            Constraint::Length(1),       // status bar
        ])
        .split(area);

    let epic_stats = app.cached_epic_stats();
    // Build the ColumnLayout once per frame (4 sorts total) so both
    // render_summary and the column-item building can share the result.
    let layout = ColumnLayout::build(app, &epic_stats);
    render_top_indicators(frame, app, vertical[0]);
    render_summary(frame, app, &layout, vertical[1]);
    // Immutable phase: compute all column rendering data while `layout` is alive.
    // Both `app` (&App reborrow) and `layout` (&ColumnLayout, which holds &App)
    // are immutable borrows — Rust allows multiple simultaneous immutable borrows.
    let cols_data = compute_columns_data(app, &layout, &epic_stats, vertical[2], now);
    // `layout` is last used above; its borrow on `app` ends here (NLL),
    // allowing the mutable list-state updates in render_columns.
    render_columns(frame, app, cols_data);
    render_input_form_panel(frame, app, vertical[3]);
    render_status_bar(frame, app, vertical[4]);

    render_error_popup(frame, app, area);
    render_help_overlay(frame, app, area);
    render_repo_filter_overlay(frame, app, area);
    render_tips_overlay(frame, app, area);
    render_task_detail_overlay(frame, app, area);
    render_learnings(frame, app, area);
    render_todos(frame, app, area);
    render_reparent_epic_overlay(frame, app, area);
    render_move_task_overlay(frame, app, area);
    render_managed_feed_config_overlay(frame, app, area);
}

/// Returns the layout constraints for the summary row based on which column is focused.
/// When an edge column (Projects=0 or Archive=5) is focused, 5 segments are shown.
/// When a task column (1–4) is focused, 4 segments are shown (task columns only).
fn column_layout_constraints(selected_col: usize) -> Vec<Constraint> {
    let n = if is_edge_column(selected_col) {
        5u32
    } else {
        4u32
    };
    vec![Constraint::Ratio(1, n); n as usize]
}

/// Layout constraints for the kanban board: content columns interleaved with
/// 1-char separator columns. Separators are at odd indices, content at even.
/// Returns 7 constraints for 4 task columns (normal) or 9 for 5 (edge column visible).
/// Epic view is handled by the caller — it constrains `selected_col` to 1–4.
pub(super) fn board_column_constraints(selected_col: usize) -> Vec<Constraint> {
    let n = if is_edge_column(selected_col) {
        5u32
    } else {
        4u32
    };
    let mut constraints = Vec::with_capacity((n * 2 - 1) as usize);
    for i in 0..n {
        if i > 0 {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Ratio(1, n));
    }
    constraints
}

pub(super) fn render_column_separator(frame: &mut Frame, area: Rect) {
    if area.width == 0 {
        return;
    }
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        buf[(area.x, y)]
            .set_symbol("\u{2502}") // │
            .set_style(Style::default().fg(BORDER));
    }
}

struct SummarySegment {
    label: String,
    color: Color,
    is_focused: bool,
    checkbox: CheckboxInfo,
}

enum CheckboxInfo {
    Task {
        all_selected: bool,
        on_select_all: bool,
        status: TaskStatus,
    },
    None,
}

fn render_summary(frame: &mut Frame, app: &App, layout: &ColumnLayout, area: Rect) {
    let sel = app.selected_column();
    let constraints = column_layout_constraints(sel);
    let col_segments = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    let segments = build_summary_segments(app, layout, sel);

    debug_assert_eq!(
        segments.len(),
        col_segments.len(),
        "summary segment count must match layout constraint count"
    );
    for (i, seg) in segments.iter().enumerate() {
        render_summary_segment(frame, seg, col_segments[i]);
    }
}

fn build_summary_segments(app: &App, layout: &ColumnLayout, sel: usize) -> Vec<SummarySegment> {
    let mut segments: Vec<SummarySegment> = Vec::new();

    for (idx, &status) in TaskStatus::ALL.iter().enumerate() {
        let is_focused = sel == idx + 1;
        segments.push(task_column_segment(app, layout, status, is_focused));
    }

    if sel == TaskStatus::COLUMN_COUNT + 1 {
        let count = app.archived_tasks().len();
        segments.push(SummarySegment {
            label: format!("\u{25b8} Archive {}", count),
            color: ARCHIVE_STRIPE,
            is_focused: true,
            checkbox: CheckboxInfo::None,
        });
    }

    segments
}

fn task_column_segment(
    app: &App,
    layout: &ColumnLayout,
    status: TaskStatus,
    is_focused: bool,
) -> SummarySegment {
    let items = layout.get(status);
    let count = items.iter().filter(|i| i.is_selectable()).count();
    let color = column_color(status);
    let prefix = if is_focused { "\u{25b8} " } else { "\u{25e6} " };
    let label = format!("{}{} {}", prefix, status.as_str(), count);

    let checkbox = if is_focused {
        let selectable = items.iter().filter(|i| i.is_selectable());
        let (n, all_selected) = selectable.fold((0usize, true), |(n, all), item| {
            let selected = match item {
                ColumnItem::Task(t) => app.selected_tasks().contains(&t.id),
                ColumnItem::Epic(e) => app.selected_epics().contains(&e.id),
                ColumnItem::EpicHeader(_)
                | ColumnItem::SubstatusLabel(_)
                | ColumnItem::OrphanSeparator => unreachable!(),
            };
            (n + 1, all && selected)
        });
        CheckboxInfo::Task {
            all_selected: n > 0 && all_selected,
            on_select_all: app.on_select_all(),
            status,
        }
    } else {
        CheckboxInfo::None
    };

    SummarySegment {
        label,
        color,
        is_focused,
        checkbox,
    }
}

fn render_summary_segment(frame: &mut Frame, seg: &SummarySegment, area: Rect) {
    let label_style = if seg.is_focused {
        Style::default()
            .fg(seg.color)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().fg(MUTED)
    };

    let spans = match &seg.checkbox {
        CheckboxInfo::Task {
            all_selected,
            on_select_all,
            status,
        } => {
            let checkbox = if *all_selected { " [x]" } else { " [ ]" };
            let checkbox_style = if *on_select_all {
                Style::default()
                    .bg(cursor_bg_color(*status))
                    .fg(FG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            };
            vec![
                Span::styled(seg.label.clone(), label_style),
                Span::styled(checkbox, checkbox_style),
            ]
        }
        CheckboxInfo::None => {
            vec![Span::styled(seg.label.clone(), label_style)]
        }
    };

    let paragraph = Paragraph::new(Line::from(spans)).alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

fn render_input_form_panel(frame: &mut Frame, app: &App, area: Rect) {
    if render_input_form(frame, app, area) {
        return;
    }
    // Empty panel — just a top border separator when no input form is active
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(BORDER));
    frame.render_widget(Paragraph::new("").block(block), area);
}

pub(super) fn wrapped_line_count(text: &str, width: usize) -> usize {
    if width == 0 {
        return 0;
    }
    text.lines()
        .map(|line| {
            if line.is_empty() {
                1
            } else {
                line.len().div_ceil(width)
            }
        })
        .sum()
}

fn render_input_form(frame: &mut Frame, app: &App, area: Rect) -> bool {
    let completed = Style::default().fg(Color::White);
    let active = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let hint = Style::default().fg(Color::DarkGray);

    let lines: Vec<Line> = match &app.input.mode {
        InputMode::InputTitle => input_title_lines(app, area, active, hint),
        InputMode::InputTag => input_tag_lines(app, completed, active, hint),
        InputMode::InputDescription => input_description_lines(app, completed, active, hint),
        InputMode::InputRepoPath => input_repo_path_lines(app, area, completed, active, hint),
        InputMode::InputBaseBranch => input_base_branch_lines(app, area, completed, active, hint),
        InputMode::InputWrapUpMode => input_wrap_up_mode_lines(app, completed, active, hint),
        InputMode::QuickDispatch => quick_dispatch_lines(app, area, active, hint),
        InputMode::MainSessionDir => main_session_dir_lines(app, area, active, hint),
        InputMode::ConfirmRetry(id) => confirm_retry_lines(app, *id),
        InputMode::InputEpicTitle => input_epic_title_lines(app, area, active, hint),
        InputMode::InputEpicDescription => {
            input_epic_description_lines(app, completed, active, hint)
        }
        _ => return false,
    };

    let is_epic_input = matches!(
        app.input.mode,
        InputMode::InputEpicTitle | InputMode::InputEpicDescription
    );

    let block_title = match &app.input.mode {
        InputMode::QuickDispatch => " Quick Dispatch ",
        InputMode::MainSessionDir => " Main Session ",
        InputMode::ConfirmRetry(_) => " Retry Agent ",
        _ if is_epic_input => " New Epic ",
        _ => " New Task ",
    };

    let border_color = match &app.input.mode {
        InputMode::ConfirmRetry(_) => Color::Red,
        _ if is_epic_input => Color::Magenta,
        _ => Color::Yellow,
    };

    let block = Block::default()
        .title(block_title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
    true
}

/// Build context-sensitive keybinding hint spans for the status bar.
/// Returns styled spans showing available actions for the selected task.
pub(in crate::tui) fn action_hints(
    task: Option<&Task>,
    _selected_column: usize,
    key_color: Color,
) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(MUTED);

    let mut spans: Vec<Span<'static>> = Vec::new();

    let mut push_hint = |key: &'static str, label: &'static str| {
        push_hint_spans(&mut spans, key, label, key_color, label_style);
    };

    if let Some(task) = task {
        match task.status {
            TaskStatus::Backlog => {
                let space_label = if task.plan_path.is_some() {
                    "dispatch"
                } else {
                    "brainstorm"
                };
                push_hint("Space", space_label);
                push_hint("e", "edit");
                push_hint("L", "move");
                push_hint("x", "done");
            }
            TaskStatus::Running => {
                if task.tmux_window.is_some() {
                    push_hint("Space", "session");
                } else if task.worktree.is_some() {
                    push_hint("Space", "resume");
                }
                push_hint("e", "edit");
                push_hint("L", "move");
                push_hint("H", "back");
                push_hint("x", "done");
            }
            TaskStatus::Review => {
                if task.worktree.is_some() {
                    push_hint("W", "wrap up");
                }
                if task.tmux_window.is_some() {
                    push_hint("Space", "session");
                    push_hint("T", "detach");
                } else if task.worktree.is_some() {
                    push_hint("Space", "resume");
                }
                push_hint("e", "edit");
                push_hint("L", "move");
                push_hint("H", "back");
                push_hint("x", "done");
            }
            TaskStatus::Done => {
                push_hint("e", "edit");
                push_hint("H", "back");
                push_hint("x", "archive");
            }
            TaskStatus::Archived => {}
        }
        if task.url.is_some() {
            push_hint("p", "open URL");
        }
    }

    if task.is_some() {
        push_hint("Enter", "detail");
        push_hint("c", "copy");
    }
    push_hint("a", "select all");
    push_hint("n", "new");
    push_hint("E", "epic");
    push_hint("D", "quick");
    push_hint("s", "split");
    push_hint("F", "flat");
    push_hint("f", "filter");
    push_hint("/", "search");
    push_hint("I", "learnings");
    push_hint("P", "todo");
    push_hint("t", "add");
    push_hint("?", "help");

    spans
}

/// Build context-sensitive keybinding hints for a selected epic.
pub(in crate::tui) fn epic_action_hints(epic: &Epic, key_color: Color) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(MUTED);

    let mut spans: Vec<Span<'static>> = Vec::new();

    let mut push_hint = |key: &'static str, label: &'static str| {
        push_hint_spans(&mut spans, key, label, key_color, label_style);
    };

    push_hint("Space", "board");
    push_hint("Enter", "detail");
    push_hint("e", "edit");
    push_hint("W", "wrap up");
    push_hint("U", "auto dispatch");
    if epic.feed_command.is_some() {
        push_hint("r", "refresh");
    }
    push_hint("L", "status \u{2192}");
    push_hint("H", "status \u{2190}");
    push_hint("x", "archive");

    push_hint("a", "select all");
    push_hint("n", "new");
    push_hint("E", "epic");
    push_hint("D", "quick");
    push_hint("F", "flat");
    push_hint("f", "filter");
    push_hint("/", "search");
    push_hint("?", "help");

    spans
}
