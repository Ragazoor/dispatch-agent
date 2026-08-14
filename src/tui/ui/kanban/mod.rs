//! Kanban board rendering: top-level entry point, summary/status bar, and
//! shared color helpers. Card, column, popup, and project-panel rendering
//! live in sibling sub-modules.

mod cards;
mod columns;
mod popups;
mod status_bar;

pub(in crate::tui) use popups::build_reparent_tree;
#[cfg(test)]
pub(in crate::tui) use status_bar::repo_drift_segment;
pub(in crate::tui) use status_bar::repo_sync_prompt_text;
#[cfg(test)]
pub(in crate::tui) use status_bar::{repo_path_for_prompt, REPO_PATH_DISPLAY_BUDGET};

#[cfg(test)]
mod tests;

use super::input_form::{
    confirm_retry_lines, input_base_branch_lines, input_description_lines,
    input_epic_description_lines, input_epic_title_lines, input_repo_path_lines, input_tag_lines,
    input_title_lines, input_wrap_up_mode_lines, main_session_dir_lines, quick_dispatch_lines,
};
use super::palette::{
    BLUE, BOARD_GROUND, BOARD_GROUND_FOCUSED, BORDER, CARD_BORDER, CARD_SURFACE, CYAN, FG, GREEN,
    MUTED, PURPLE, RED, YELLOW,
};
use super::shared::{push_hint_spans, render_top_indicators, rounded_block};
use super::todos::render_todos;

use crate::models::{Epic, Task, TaskStatus};
use crate::tui::{is_edge_column, App, ColumnItem, ColumnLayout, InputMode};
use chrono::Utc;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use columns::{compute_columns_data, render_columns};
use popups::{
    render_error_popup, render_help_overlay, render_move_task_overlay,
    render_reparent_epic_overlay, render_repo_filter_overlay, render_task_detail_overlay,
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

/// Highlight fill for the select-all checkbox in a focused column header.
///
/// This is the *header* checkbox, not a card: it is the one place a tinted fill
/// still tracks the column's identity colour. Cards no longer take a tinted
/// fill at all — see [`card_surface_color`].
pub(in crate::tui) fn select_all_highlight_bg(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => Color::Rgb(42, 48, 82),
        TaskStatus::Running => Color::Rgb(78, 62, 32),
        TaskStatus::Review => Color::Rgb(62, 42, 82),
        TaskStatus::Done => Color::Rgb(38, 64, 42),
        TaskStatus::Archived => Color::Rgb(42, 48, 82),
    }
}

/// Neutral ground for a column, uniform across every column.
///
/// The `status` parameter is deliberately unused: `core.allium` ("Column ground
/// and card surface") makes the ground *the same colour in every column* at a
/// given focus state, so binding it under a leading underscore is what
/// compiler-enforces that no identity hue can leak back in. It is retained so
/// call sites read symmetrically and so `board_ground_is_uniform_across_columns`
/// in `src/tui/tests/rendering.rs` has something to vary.
///
/// Focus raises the ground's lightness without tinting it
/// (`NeutralRampIsStrictlyAscending`).
pub(in crate::tui) fn column_bg_color(_status: TaskStatus, is_focused: bool) -> Color {
    if is_focused {
        BOARD_GROUND_FOCUSED
    } else {
        BOARD_GROUND
    }
}

/// The fill every card is drawn on — the top of the neutral ramp.
pub(in crate::tui) fn card_surface_color() -> Color {
    CARD_SURFACE
}

/// The fill a *selected* card is drawn on.
///
/// Equal to [`card_surface_color`] by design (`core.allium` invariant
/// `SelectionDoesNotLiftTheFill`): selection is carried by frame hue and title
/// weight, not by a lighter fill. Kept as its own function so the equality is
/// something a test can assert rather than something a reader has to infer.
pub(in crate::tui) fn selected_card_surface_color() -> Color {
    CARD_SURFACE
}

/// A resting card's frame colour. Neutral — the frame takes the column's
/// identity colour only while the card is selected.
pub(in crate::tui) fn card_border_color() -> Color {
    CARD_BORDER
}

/// Fill color for a column's header bar, tinted to its identity color.
///
/// Focus is signalled by intensity, never by presence-of-color: the unfocused
/// fill is a weaker wash of the same hue, not grey (`core.allium`: "Focus is
/// intensity, not colour-vs-absence").
pub(in crate::tui) fn column_header_bg(status: TaskStatus, is_focused: bool) -> Color {
    if is_focused {
        match status {
            TaskStatus::Backlog => Color::Rgb(45, 58, 102),
            TaskStatus::Running => Color::Rgb(86, 66, 30),
            TaskStatus::Review => Color::Rgb(74, 47, 102),
            TaskStatus::Done => Color::Rgb(46, 74, 40),
            TaskStatus::Archived => Color::Rgb(48, 54, 82),
        }
    } else {
        match status {
            TaskStatus::Backlog => Color::Rgb(35, 40, 66),
            TaskStatus::Running => Color::Rgb(61, 53, 32),
            TaskStatus::Review => Color::Rgb(52, 38, 68),
            TaskStatus::Done => Color::Rgb(36, 58, 40),
            TaskStatus::Archived => Color::Rgb(34, 38, 54),
        }
    }
}

/// Label color for a column's header bar.
///
/// Unfocused keeps the column's identity color; focused brightens toward the
/// foreground so the focused column reads as the one at full strength.
pub(in crate::tui) fn column_header_fg(status: TaskStatus, is_focused: bool) -> Color {
    if is_focused {
        match status {
            TaskStatus::Backlog => Color::Rgb(208, 224, 255),
            TaskStatus::Running => Color::Rgb(255, 230, 190),
            TaskStatus::Review => Color::Rgb(230, 212, 255),
            TaskStatus::Done => Color::Rgb(216, 244, 190),
            TaskStatus::Archived => Color::Rgb(200, 208, 236),
        }
    } else {
        column_color(status)
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
        let block = rounded_block(border_color);
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
    render_task_detail_overlay(frame, app, area);
    render_todos(frame, app, area);
    render_reparent_epic_overlay(frame, app, area);
    render_move_task_overlay(frame, app, area);
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
    /// Selectable-item count, rendered after the label at reduced emphasis.
    count: String,
    /// Header-bar fill and label colors, resolved per column identity + focus
    /// (`core.allium`: "Column header bar").
    header_bg: Color,
    header_fg: Color,
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
            label: "\u{25b8} ARCHIVE".to_string(),
            count: format!(" {count}"),
            header_bg: column_header_bg(TaskStatus::Archived, true),
            header_fg: column_header_fg(TaskStatus::Archived, true),
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
    let prefix = if is_focused { "\u{25b8} " } else { "\u{25e6} " };
    // Label uppercased, count carried separately so it can render at reduced
    // emphasis (core.allium: "Column header bar").
    let label = format!("{}{}", prefix, status.as_str().to_uppercase());

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
        count: format!(" {count}"),
        header_bg: column_header_bg(status, is_focused),
        header_fg: column_header_fg(status, is_focused),
        is_focused,
        checkbox,
    }
}

fn render_summary_segment(frame: &mut Frame, seg: &SummarySegment, area: Rect) {
    // The header is a filled bar in the column's identity color; focus is
    // carried by the fill/label intensity and bold, never by dropping the hue
    // (core.allium: "Focus is intensity, not colour-vs-absence").
    let bar_style = Style::default().bg(seg.header_bg);
    let mut label_style = bar_style.fg(seg.header_fg);
    if seg.is_focused {
        label_style = label_style.add_modifier(Modifier::BOLD);
    }
    // Count sits at reduced emphasis against the same fill.
    let count_style = bar_style.fg(dim_against(seg.header_fg, seg.header_bg));

    let mut spans = vec![
        Span::styled(seg.label.clone(), label_style),
        Span::styled(seg.count.clone(), count_style),
    ];
    if let CheckboxInfo::Task {
        all_selected,
        on_select_all,
        status,
    } = &seg.checkbox
    {
        let checkbox = if *all_selected { " [x]" } else { " [ ]" };
        let checkbox_style = if *on_select_all {
            bar_style
                .bg(select_all_highlight_bg(*status))
                .fg(FG)
                .add_modifier(Modifier::BOLD)
        } else {
            count_style
        };
        spans.push(Span::styled(checkbox, checkbox_style));
    }

    // Paint the whole segment with the bar fill first so the tint runs edge to
    // edge, then draw the centred label over it.
    frame.render_widget(Block::default().style(bar_style), area);
    let paragraph = Paragraph::new(Line::from(spans))
        .style(bar_style)
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

/// Midpoint between a label color and its background — used for the header's
/// item count, which must read as secondary to the label without losing the
/// column's hue.
fn dim_against(fg: Color, bg: Color) -> Color {
    match (fg, bg) {
        (Color::Rgb(fr, fg_, fb), Color::Rgb(br, bg_, bb)) => Color::Rgb(
            ((u16::from(fr) + u16::from(br)) / 2) as u8,
            ((u16::from(fg_) + u16::from(bg_)) / 2) as u8,
            ((u16::from(fb) + u16::from(bb)) / 2) as u8,
        ),
        _ => MUTED,
    }
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
    let completed = Style::default().fg(FG);
    let active = Style::default().fg(YELLOW).add_modifier(Modifier::BOLD);
    let hint = Style::default().fg(MUTED);

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
        InputMode::ConfirmRetry(_) => RED,
        _ if is_epic_input => PURPLE,
        _ => YELLOW,
    };

    let block = rounded_block(border_color).title(block_title);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
    true
}

/// Build context-sensitive keybinding hint spans for the status bar.
/// Returns styled spans showing available actions for the selected task.
///
/// `dispatch_in_flight` comes from [`crate::tui::App::dispatch_may_be_in_flight`].
/// It is not cosmetic: an unprovisioned task is indistinguishable from one
/// mid-provisioning, and advertising retry on the latter invites a second
/// dispatch. See `RetryReachableInPlace` in `docs/specs/dispatch.allium`.
pub(in crate::tui) fn action_hints(
    task: Option<&Task>,
    dispatch_in_flight: bool,
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
                } else if !dispatch_in_flight {
                    // Unprovisioned: nothing to jump to or resume, but Space
                    // opens the kill-and-retry dialog. See RetryReachableInPlace
                    // in docs/specs/dispatch.allium.
                    push_hint("Space", "retry");
                }
                push_hint("e", "edit");
                push_hint("L", "move");
                push_hint("H", "back");
                push_hint("x", "done");
            }
            TaskStatus::Review => {
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

    // `Space` on an epic card enters the epic (`EpicMessage::Enter`). `U` is deliberately
    // absent: it needs `current_epic_id()`, so it only works from inside the epic view,
    // where the header badge advertises it instead.
    push_hint("Space", "enter");
    push_hint("Enter", "detail");
    push_hint("e", "edit");
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
