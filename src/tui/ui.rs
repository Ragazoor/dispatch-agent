use chrono::{DateTime, Utc};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use super::{App, ColumnItem, ColumnLayout, InputMode, RepoFilterMode, ReviewBoardMode, ViewMode};
use crate::dispatch;
use crate::models::{
    epic_substatus, format_age, CiStatus, Epic, ReviewDecision, ReviewPr, Staleness, SubStatus,
    Task, TaskId, TaskStatus, TaskUsage,
};

// ── Tokyo Night palette ─────────────────────────────────────────────
const MUTED: Color = Color::Rgb(86, 95, 137);
const MUTED_LIGHT: Color = Color::Rgb(120, 124, 153);
const FG: Color = Color::Rgb(192, 202, 245);
const BORDER: Color = Color::Rgb(41, 46, 66);
const YELLOW: Color = Color::Rgb(224, 175, 104);
const PURPLE: Color = Color::Rgb(187, 154, 247);
const GREEN: Color = Color::Rgb(158, 206, 106);
const CYAN: Color = Color::Rgb(86, 182, 194);
const RED_DIM: Color = Color::Rgb(224, 130, 130);
const FLASH_BG: Color = Color::Rgb(62, 52, 20);

/// Column color per status
fn column_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => MUTED,
        TaskStatus::Running => YELLOW,
        TaskStatus::Review => PURPLE,
        TaskStatus::Done => GREEN,
        TaskStatus::Archived => MUTED,
    }
}

/// Tinted background for the cursor card in each column.
fn cursor_bg_color(status: TaskStatus) -> Color {
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
fn column_bg_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => Color::Rgb(28, 30, 44),
        TaskStatus::Running => Color::Rgb(38, 34, 26),
        TaskStatus::Review => Color::Rgb(34, 28, 44),
        TaskStatus::Done => Color::Rgb(27, 36, 30),
        TaskStatus::Archived => Color::Rgb(28, 30, 44),
    }
}

/// Unicode status icon for the metadata line of each card.
fn status_icon(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Backlog => "◦",
        TaskStatus::Running => "◉",
        TaskStatus::Review => "◎",
        TaskStatus::Done => "✓",
        TaskStatus::Archived => "◦",
    }
}

fn truncate_for_detail(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

/// Map a staleness tier to a terminal color.
/// Uses indexed terminal colors (not palette constants) so these adapt to the
/// user's terminal theme rather than being locked to Tokyo Night RGB values.
fn staleness_color(staleness: Staleness) -> Color {
    match staleness {
        Staleness::Fresh => Color::Green,
        Staleness::Aging => Color::Yellow,
        Staleness::Stale => Color::Red,
    }
}

/// Truncate a string to at most `max` characters, appending "…" if truncated.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

/// Compute how tall the detail/input panel should be based on the current input mode.
/// Expands when a repo list is being shown so all repos (plus cursor) are visible.
fn input_panel_height(app: &App, area_height: u16, has_banner: bool) -> u16 {
    // Fixed overhead: tab_bar(1) + summary(1) + kanban_min(6) + status_bar(1) = 9,
    // plus epic banner(4) when present.
    let overhead: u16 = if has_banner { 13 } else { 9 };
    let max_height = area_height.saturating_sub(overhead).max(8);
    match &app.input.mode {
        InputMode::QuickDispatch => {
            // header(1) + blank(1) + repos(N) + blank(1) + hint(1) + borders(2) = N + 6
            let rows = app.repo_paths.len() as u16 + 6;
            rows.clamp(8, max_height)
        }
        InputMode::InputRepoPath | InputMode::InputEpicRepoPath if app.input.buffer.is_empty() => {
            // title(1) + desc(1) + path_input(1) + repos(N) + blank(1) + hint(1) + borders(2) = N + 7
            let rows = app.repo_paths.len() as u16 + 7;
            rows.clamp(8, max_height)
        }
        _ => 8,
    }
}

/// Top-level render function.
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let now = Utc::now();

    if matches!(app.view_mode(), ViewMode::ReviewBoard { .. }) {
        render_review_board(frame, app, area);
        if matches!(app.mode(), InputMode::Help) {
            render_help_overlay(frame, app, area);
        }
        render_error_popup(frame, app, area);
        return;
    }

    if matches!(app.view_mode(), ViewMode::SecurityBoard { .. }) {
        render_security_board(frame, app, area);
        if matches!(app.mode(), InputMode::Help) {
            render_help_overlay(frame, app, area);
        }
        render_error_popup(frame, app, area);
        return;
    }

    let has_banner = matches!(app.view_mode(), ViewMode::Epic { .. });

    let panel_h = input_panel_height(app, area.height, has_banner);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if has_banner {
            vec![
                Constraint::Length(1),       // tab bar
                Constraint::Length(1),       // summary row
                Constraint::Length(4),       // epic banner
                Constraint::Min(6),          // kanban board
                Constraint::Length(panel_h), // detail panel
                Constraint::Length(1),       // status bar
            ]
        } else {
            vec![
                Constraint::Length(1),       // tab bar
                Constraint::Length(1),       // summary row
                Constraint::Min(6),          // kanban board
                Constraint::Length(panel_h), // detail panel
                Constraint::Length(1),       // status bar
            ]
        })
        .split(area);

    if has_banner {
        render_tab_bar(frame, app, vertical[0]);
        render_summary(frame, app, vertical[1]);
        render_epic_banner(frame, app, vertical[2]);
        render_columns(frame, app, vertical[3], now);
        render_archive_overlay(frame, app, vertical[3], now);
        render_detail(frame, app, vertical[4], now);
        render_status_bar(frame, app, vertical[5]);
    } else {
        render_tab_bar(frame, app, vertical[0]);
        render_summary(frame, app, vertical[1]);
        render_columns(frame, app, vertical[2], now);
        render_archive_overlay(frame, app, vertical[2], now);
        render_detail(frame, app, vertical[3], now);
        render_status_bar(frame, app, vertical[4]);
    }

    render_error_popup(frame, app, area);
    render_help_overlay(frame, app, area);
    render_repo_filter_overlay(frame, app, area);
}

fn review_tab_label(app: &App, prefix: &str) -> String {
    let review_count = app.review_prs().len();
    let loading = if app.review_board_loading() {
        " \u{21bb}"
    } else {
        ""
    };
    if review_count > 0 {
        format!("{prefix}Reviews ({review_count}){loading} ")
    } else {
        format!("{prefix}Reviews{loading} ")
    }
}

fn my_prs_tab_label(app: &App, prefix: &str) -> String {
    let my_count = app.my_prs().len();
    let loading = if app.my_prs_loading() {
        " \u{21bb}"
    } else {
        ""
    };
    let filter = if app.dispatch_pr_filter() {
        " \u{25c6}"
    } else {
        ""
    };
    if my_count > 0 {
        format!("{prefix}My PRs ({my_count}){filter}{loading} ")
    } else {
        format!("{prefix}My PRs{filter}{loading} ")
    }
}

fn bot_prs_tab_label(app: &App, prefix: &str) -> String {
    let bot_count = app.bot_prs().len();
    let loading = if app.bot_prs_loading() {
        " \u{21bb}"
    } else {
        ""
    };
    if bot_count > 0 {
        format!("{prefix}Dependabot ({bot_count}){loading} ")
    } else {
        format!("{prefix}Dependabot{loading} ")
    }
}

fn render_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    let active_style = Style::default().fg(FG).add_modifier(Modifier::BOLD);
    let inactive_style = Style::default().fg(MUTED);
    let hint_style = Style::default().fg(MUTED);

    let mut spans: Vec<Span> = Vec::new();

    match app.view_mode() {
        ViewMode::Epic { epic_id, .. } => {
            let epic_title = app
                .epics()
                .iter()
                .find(|e| e.id == *epic_id)
                .map(|e| truncate(&e.title, 30))
                .unwrap_or_else(|| "Epic".to_string());
            spans.push(Span::styled(
                format!(" \u{25b8} Epic: {epic_title} "),
                active_style.fg(PURPLE),
            ));
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(BORDER)));
            spans.push(Span::styled(" Tasks ", inactive_style));
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(BORDER)));
            spans.push(Span::styled(review_tab_label(app, " "), inactive_style));
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(BORDER)));
            spans.push(Span::styled(security_tab_label(app, " "), inactive_style));
        }
        ViewMode::Board(_) => {
            spans.push(Span::styled(" \u{25b8} Tasks ", active_style));
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(BORDER)));
            spans.push(Span::styled(review_tab_label(app, " "), inactive_style));
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(BORDER)));
            spans.push(Span::styled(security_tab_label(app, " "), inactive_style));
        }
        ViewMode::ReviewBoard { mode, .. } => {
            spans.push(Span::styled(" Tasks ", inactive_style));
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(BORDER)));
            let label = match mode {
                ReviewBoardMode::Reviewer => review_tab_label(app, " \u{25b8} "),
                ReviewBoardMode::Author => my_prs_tab_label(app, " \u{25b8} "),
                ReviewBoardMode::Dependabot => bot_prs_tab_label(app, " \u{25b8} "),
            };
            spans.push(Span::styled(label, active_style));
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(BORDER)));
            spans.push(Span::styled(security_tab_label(app, " "), inactive_style));
        }
        ViewMode::SecurityBoard { .. } => {
            spans.push(Span::styled(" Tasks ", inactive_style));
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(BORDER)));
            spans.push(Span::styled(review_tab_label(app, " "), inactive_style));
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(BORDER)));
            spans.push(Span::styled(
                security_tab_label(app, " \u{25b8} "),
                active_style,
            ));
        }
    }

    let hint = match app.view_mode() {
        ViewMode::ReviewBoard { .. } => "  [Tab] security  [S-Tab] toggle",
        ViewMode::SecurityBoard { .. } => "  [Tab] tasks  [Esc] back",
        _ => "  [Tab]",
    };
    spans.push(Span::styled(hint, hint_style.add_modifier(Modifier::BOLD)));

    let line = Line::from(spans);
    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);

    // Right-aligned indicators (filter, notifications)
    let mut right_parts: Vec<Span> = Vec::new();
    if !app.repo_filter().is_empty() {
        let active = app.repo_filter().len();
        let total = app.repo_paths().len();
        let label = match app.repo_filter_mode() {
            RepoFilterMode::Include => format!("[{active}/{total} repos]  "),
            RepoFilterMode::Exclude => format!("[excl {active}/{total} repos]  "),
        };
        right_parts.push(Span::styled(label, Style::default().fg(MUTED)));
    }
    if app.notifications_enabled() {
        right_parts.push(Span::styled(
            "\u{1F514}",
            Style::default().fg(Color::Yellow),
        ));
    } else {
        right_parts.push(Span::styled("\u{1F515} [N]", Style::default().fg(MUTED)));
    }
    if !right_parts.is_empty() {
        let right_line = Line::from(right_parts);
        let p = Paragraph::new(right_line).alignment(Alignment::Right);
        frame.render_widget(p, area);
    }
}

fn render_summary(frame: &mut Frame, app: &App, area: Rect) {
    let col_segments = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [Constraint::Ratio(1, TaskStatus::COLUMN_COUNT as u32); TaskStatus::COLUMN_COUNT],
        )
        .split(area);

    let layout = ColumnLayout::build(app);

    for (col_idx, &status) in TaskStatus::ALL.iter().enumerate() {
        let items = layout.get(status);
        let count = items.len();
        let is_focused = app.selected_column() == col_idx;
        let color = column_color(status);

        let (prefix, label_style) = if is_focused {
            (
                "\u{25b8} ",
                Style::default()
                    .fg(color)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            )
        } else {
            ("\u{25e6} ", Style::default().fg(MUTED))
        };

        let label = format!("{}{} {}", prefix, status.as_str(), count);

        let spans = if is_focused {
            let all_selected = !items.is_empty()
                && items.iter().all(|item| match item {
                    ColumnItem::Task(t) => app.selected_tasks().contains(&t.id),
                    ColumnItem::Epic(e) => app.selected_epics().contains(&e.id),
                });
            let checkbox = if all_selected { " [x]" } else { " [ ]" };

            let checkbox_style = if app.on_select_all() {
                Style::default()
                    .bg(cursor_bg_color(status))
                    .fg(FG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            };

            vec![
                Span::styled(label, label_style),
                Span::styled(checkbox, checkbox_style),
            ]
        } else {
            vec![Span::styled(label, label_style)]
        };

        let paragraph = Paragraph::new(Line::from(spans)).alignment(Alignment::Center);
        frame.render_widget(paragraph, col_segments[col_idx]);
    }
}

/// Format the title text for a task card (line 1 only — status annotations are on line 2).
fn format_task_title(task: &Task, max_title: usize) -> String {
    truncate(&task.title, max_title)
}

// ---------------------------------------------------------------------------
// CardIndicator — what to show on line 2 of a task card
// ---------------------------------------------------------------------------

/// Classifies a task's current state into a single display indicator.
/// Priority order matters: conflict > detached-review > crashed > stale >
/// blocked > detached-running > running > review-pr > done-merged > idle.
enum CardIndicator {
    Conflict,
    DetachedReview {
        pr_label: String,
    },
    Detached,
    Crashed,
    Stale {
        inactive_mins: u64,
    },
    Blocked,
    Running,
    ReviewPr {
        pr_label: String,
    },
    DoneMerged {
        pr_label: String,
    },
    Idle {
        status: TaskStatus,
        age: String,
        staleness: Staleness,
        plan_indicator: &'static str,
        tag_suffix: &'static str,
    },
}

fn classify_card_indicator(
    task: &Task,
    status: TaskStatus,
    app: &App,
    now: DateTime<Utc>,
) -> CardIndicator {
    if task.sub_status == SubStatus::Conflict {
        return CardIndicator::Conflict;
    }
    if task.is_detached() {
        if let (TaskStatus::Review, Some(pr_url)) = (status, task.pr_url.as_deref()) {
            let pr_label = crate::models::pr_number_from_url(pr_url)
                .map_or("PR".to_string(), |n| format!("PR #{n}"));
            return CardIndicator::DetachedReview { pr_label };
        }
        return CardIndicator::Detached;
    }
    if task.sub_status == SubStatus::Crashed {
        return CardIndicator::Crashed;
    }
    if task.sub_status == SubStatus::Stale {
        let inactive_mins = app
            .agents
            .last_output_change
            .get(&task.id)
            .map(|t| t.elapsed().as_secs() / 60)
            .unwrap_or(0);
        return CardIndicator::Stale { inactive_mins };
    }
    if status == TaskStatus::Running && task.sub_status == SubStatus::NeedsInput {
        return CardIndicator::Blocked;
    }
    if status == TaskStatus::Running {
        return CardIndicator::Running;
    }
    if let (TaskStatus::Review, Some(pr_url)) = (status, task.pr_url.as_deref()) {
        let pr_label = crate::models::pr_number_from_url(pr_url)
            .map_or("PR".to_string(), |n| format!("PR #{n}"));
        return CardIndicator::ReviewPr { pr_label };
    }
    if let (TaskStatus::Done, Some(pr_url)) = (status, task.pr_url.as_deref()) {
        let pr_label = crate::models::pr_number_from_url(pr_url)
            .map_or("PR".to_string(), |n| format!("PR #{n}"));
        return CardIndicator::DoneMerged { pr_label };
    }

    let age = format_age(task.updated_at, now);
    let staleness = Staleness::from_age(task.updated_at, now);
    let plan_indicator = if task.plan.is_some() && status == TaskStatus::Backlog {
        "▸ "
    } else {
        ""
    };
    let tag_suffix = match task.tag {
        Some(crate::models::TaskTag::Bug) => " [bug]",
        Some(crate::models::TaskTag::Feature) => " [feat]",
        Some(crate::models::TaskTag::Chore) => " [chore]",
        Some(crate::models::TaskTag::Epic) => " [epic]",
        None => "",
    };
    CardIndicator::Idle {
        status,
        age,
        staleness,
        plan_indicator,
        tag_suffix,
    }
}

fn render_card_indicator(indicator: CardIndicator) -> Line<'static> {
    let (label, color) = match indicator {
        CardIndicator::Conflict => ("\u{26a0} rebase conflict".to_string(), Color::Red),
        CardIndicator::DetachedReview { pr_label } => (format!("\u{25cb} {pr_label}"), Color::Cyan),
        CardIndicator::Detached => ("\u{25cb} detached".to_string(), MUTED),
        CardIndicator::Crashed => ("\u{26a0} crashed".to_string(), Color::Red),
        CardIndicator::Stale { inactive_mins } => (
            format!("\u{25c9} stale \u{00b7} {}m", inactive_mins),
            Color::Yellow,
        ),
        CardIndicator::Blocked => ("\u{25c9} blocked".to_string(), Color::Yellow),
        CardIndicator::Running => (
            format!("{} running", status_icon(TaskStatus::Running)),
            CYAN,
        ),
        CardIndicator::ReviewPr { pr_label } => (format!("\u{25cf} {pr_label}"), Color::Cyan),
        CardIndicator::DoneMerged { pr_label } => {
            (format!("\u{2714} {pr_label} merged"), Color::Green)
        }
        CardIndicator::Idle {
            status,
            age,
            staleness,
            plan_indicator,
            tag_suffix,
        } => {
            let icon = status_icon(status);
            (
                format!("{plan_indicator}{icon} {age}{tag_suffix}"),
                staleness_color(staleness),
            )
        }
    };
    Line::from(vec![
        Span::raw("   "),
        Span::styled(label, Style::default().fg(color)),
    ])
}

/// Build a styled two-line ListItem for a task card in a kanban column.
/// Line 1: stripe + title
/// Line 2: status icon + age/activity metadata
fn build_task_list_item<'a>(
    task: &Task,
    status: TaskStatus,
    app: &App,
    now: DateTime<Utc>,
    is_cursor: bool,
    col_color: Color,
) -> ListItem<'a> {
    let is_batch_selected = app.selected_tasks().contains(&task.id);
    let select_prefix = if is_batch_selected { "* " } else { "  " };

    let title_text = format_task_title(task, 32);

    // Line 1: prefix + stripe + title
    // Cursor gets a thicker stripe (▌) as a left accent bar
    let stripe_char = if is_cursor { "\u{258c}" } else { "\u{258e}" };
    let stripe_style = Style::default().fg(col_color);
    let title_style = if is_batch_selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let has_message_flash = app
        .agents
        .message_flash
        .get(&task.id)
        .is_some_and(|t| t.elapsed().as_secs() < 3);

    let mut line1_spans = vec![
        Span::styled(select_prefix.to_string(), title_style),
        Span::styled(stripe_char, stripe_style),
        Span::styled(format!(" #{} ", task.id), Style::default().fg(MUTED)),
        Span::styled(title_text.to_string(), title_style),
    ];
    if has_message_flash {
        line1_spans.push(Span::styled(
            " \u{2709}",
            Style::default().fg(Color::Yellow),
        ));
    }

    let line1 = Line::from(line1_spans);

    let line2 = render_card_indicator(classify_card_indicator(task, status, app, now));

    // Build two-line ListItem
    let mut item = ListItem::new(vec![line1, line2]);

    // Flash bg takes priority over cursor — it's transient (3s) and meant to grab attention
    if has_message_flash {
        item = item.style(
            Style::default()
                .bg(FLASH_BG)
                .fg(FG)
                .add_modifier(Modifier::BOLD),
        );
    } else if is_cursor {
        item = item.style(
            Style::default()
                .bg(cursor_bg_color(status))
                .fg(FG)
                .add_modifier(Modifier::BOLD),
        );
    }

    item
}

/// Non-selectable section header injected between substatus groups.
fn render_substatus_header(label: &str, col_color: Color) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(
        format!("  \u{2500}\u{2500} {label} "),
        Style::default().fg(col_color).add_modifier(Modifier::DIM),
    )))
}

fn render_columns(frame: &mut Frame, app: &mut App, area: Rect, now: DateTime<Utc>) {
    let column_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [Constraint::Ratio(1, TaskStatus::COLUMN_COUNT as u32); TaskStatus::COLUMN_COUNT],
        )
        .split(area);

    for (col_idx, &status) in TaskStatus::ALL.iter().enumerate() {
        let col_area = column_areas[col_idx];
        let is_focused = app.selected_column() == col_idx;
        let color = column_color(status);
        // Only Running and Review benefit from substatus grouping headers.
        let show_headers = matches!(status, TaskStatus::Running | TaskStatus::Review);

        let column_items = app.column_items_for_status(status);
        let selected_row = app.selected_row()[col_idx];

        let mut list_items: Vec<ListItem> = Vec::new();
        let mut list_selection_idx: Option<usize> = None;
        let mut current_priority: Option<u8> = None;

        for (item_idx, item) in column_items.iter().enumerate() {
            if show_headers {
                let priority = match item {
                    ColumnItem::Task(t) => t.sub_status.column_priority_detached(t.is_detached()),
                    ColumnItem::Epic(e) => {
                        let subtasks: Vec<Task> = app
                            .tasks()
                            .iter()
                            .filter(|t| t.epic_id == Some(e.id) && t.status != TaskStatus::Archived)
                            .cloned()
                            .collect();
                        let active_merge = app.merge_queue().map(|q| q.epic_id);
                        epic_substatus(e, &subtasks, active_merge).column_priority()
                    }
                };
                if Some(priority) != current_priority {
                    current_priority = Some(priority);
                    let label = match item {
                        ColumnItem::Task(t) => t
                            .sub_status
                            .header_label_detached(t.is_detached())
                            .to_string(),
                        ColumnItem::Epic(e) => {
                            let subtasks: Vec<Task> = app
                                .tasks()
                                .iter()
                                .filter(|t| {
                                    t.epic_id == Some(e.id) && t.status != TaskStatus::Archived
                                })
                                .cloned()
                                .collect();
                            let active_merge = app.merge_queue().map(|q| q.epic_id);
                            epic_substatus(e, &subtasks, active_merge)
                                .header_label()
                                .to_string()
                        }
                    };
                    list_items.push(render_substatus_header(&label, color));
                }
            }

            if item_idx == selected_row {
                list_selection_idx = Some(list_items.len());
            }

            let is_cursor = is_focused && !app.on_select_all() && item_idx == selected_row;
            list_items.push(match item {
                ColumnItem::Task(task) => {
                    build_task_list_item(task, status, app, now, is_cursor, color)
                }
                ColumnItem::Epic(epic) => render_epic_item(epic, is_cursor, app, status),
            });
        }

        let on_select_all = app.on_select_all();
        let sel = app.selection_mut();
        if is_focused {
            *sel.list_states[col_idx].selected_mut() = if on_select_all {
                None
            } else {
                list_selection_idx
            };
        }

        if is_focused {
            let block = Block::default().style(Style::default().bg(column_bg_color(status)));
            let inner = block.inner(col_area);
            frame.render_widget(block, col_area);
            frame.render_stateful_widget(
                List::new(list_items),
                inner,
                &mut sel.list_states[col_idx],
            );
        } else {
            frame.render_stateful_widget(
                List::new(list_items),
                col_area,
                &mut sel.list_states[col_idx],
            );
        }
    }
}

fn render_epic_item(
    epic: &Epic,
    is_cursor: bool,
    app: &App,
    status: TaskStatus,
) -> ListItem<'static> {
    let subtask_statuses: Vec<TaskStatus> = app
        .tasks()
        .iter()
        .filter(|t| t.epic_id == Some(epic.id) && t.status != TaskStatus::Archived)
        .map(|t| t.status)
        .collect();

    let done_count = subtask_statuses
        .iter()
        .filter(|s| **s == TaskStatus::Done)
        .count();
    let running_count = subtask_statuses
        .iter()
        .filter(|s| **s == TaskStatus::Running)
        .count();
    let review_count = subtask_statuses
        .iter()
        .filter(|s| **s == TaskStatus::Review)
        .count();

    let title_text = truncate(&epic.title, 28);
    let plan_indicator = if epic.plan.is_some() && status == TaskStatus::Backlog {
        " \u{25b8}" // ▸
    } else {
        ""
    };

    let is_batch_selected = app.selected_epics().contains(&epic.id);
    let select_prefix = if is_batch_selected { "* " } else { "  " };

    // Line 1: stripe + title (thicker stripe for cursor)
    let stripe_char = if is_cursor { "\u{258c}" } else { "\u{258e}" };
    let title_style = Style::default().fg(PURPLE).add_modifier(Modifier::BOLD);
    let line1 = Line::from(vec![
        Span::raw(select_prefix.to_string()),
        Span::styled(stripe_char, Style::default().fg(PURPLE)),
        Span::styled(format!(" #{} ", epic.id), Style::default().fg(MUTED)),
        Span::styled(format!("{title_text}{plan_indicator}"), title_style),
    ]);

    // Line 2: colored status indicators + substatus label
    let backlog_count = subtask_statuses
        .iter()
        .filter(|s| **s == TaskStatus::Backlog)
        .count();
    let total = subtask_statuses.len();

    let line2 = if total > 0 {
        let subtasks: Vec<Task> = app
            .tasks()
            .iter()
            .filter(|t| t.epic_id == Some(epic.id) && t.status != TaskStatus::Archived)
            .cloned()
            .collect();
        let active_merge = app.merge_queue().map(|q| q.epic_id);
        let substatus = crate::models::epic_substatus(epic, &subtasks, active_merge);

        let mut spans = vec![Span::raw("    ".to_string())];
        let indicators: &[(usize, Color)] = &[
            (backlog_count, column_color(TaskStatus::Backlog)),
            (running_count, column_color(TaskStatus::Running)),
            (review_count, column_color(TaskStatus::Review)),
            (done_count, column_color(TaskStatus::Done)),
        ];
        for (count, color) in indicators {
            if *count > 0 {
                spans.push(Span::styled(
                    format!("\u{25cf}{count} "),
                    Style::default().fg(*color),
                ));
            }
        }
        spans.push(Span::styled(substatus.label(), Style::default().fg(MUTED)));
        Line::from(spans)
    } else {
        Line::from(vec![
            Span::raw("    "),
            Span::styled("no subtasks", Style::default().fg(MUTED)),
        ])
    };

    let mut item = ListItem::new(vec![line1, line2]);

    if is_cursor {
        item = item.style(
            Style::default()
                .bg(cursor_bg_color(status))
                .fg(FG)
                .add_modifier(Modifier::BOLD),
        );
    }

    item
}

fn render_epic_banner(frame: &mut Frame, app: &App, area: Rect) {
    let ViewMode::Epic { epic_id, .. } = app.view_mode() else {
        return;
    };
    let Some(epic) = app.epics().iter().find(|e| e.id == *epic_id) else {
        return;
    };

    let subtask_statuses: Vec<TaskStatus> = app
        .tasks()
        .iter()
        .filter(|t| t.epic_id == Some(epic.id) && t.status != TaskStatus::Archived)
        .map(|t| t.status)
        .collect();
    let total = subtask_statuses.len();
    let done = subtask_statuses
        .iter()
        .filter(|s| **s == TaskStatus::Done)
        .count();

    let block = Block::default()
        .title(format!(" Epic: {} ", epic.title))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Magenta))
        .title_style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        );

    let desc = truncate(&epic.description, 60);
    let subtasks: Vec<Task> = app
        .tasks()
        .iter()
        .filter(|t| t.epic_id == Some(epic.id) && t.status != TaskStatus::Archived)
        .cloned()
        .collect();
    let active_merge = app.merge_queue().map(|q| q.epic_id);
    let substatus = crate::models::epic_substatus(epic, &subtasks, active_merge);
    let progress = format!("{done}/{total} done \u{00b7} {}", substatus.label());
    let lines = vec![
        Line::from(vec![
            Span::styled(desc, Style::default().fg(Color::Gray)),
            Span::styled(
                format!("  {progress}"),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(Span::styled(
            "Esc to return to board",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn render_archive_overlay(frame: &mut Frame, app: &mut App, area: Rect, now: DateTime<Utc>) {
    if !app.show_archived() {
        return;
    }

    let archived = app.archived_tasks();

    // Right-side overlay: 40% of screen width, full height of kanban area
    let overlay_width = (area.width * 40 / 100).clamp(30, 60);
    let x = area.x + area.width.saturating_sub(overlay_width);
    let overlay_area = Rect::new(x, area.y, overlay_width, area.height);

    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .title(format!(" Archive ({}) ", archived.len()))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::DarkGray))
        .title_style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let items: Vec<ListItem> = archived
        .iter()
        .enumerate()
        .map(|(idx, task)| {
            let age = format_age(task.updated_at, now);
            let title = truncate(&task.title, (overlay_width as usize).saturating_sub(10));
            let label = format!("{title} {age}");
            let is_selected = idx == app.selected_archive_row();
            if is_selected {
                ListItem::new(Line::from(Span::styled(
                    label,
                    Style::default()
                        .bg(Color::DarkGray)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )))
            } else {
                ListItem::new(Line::from(Span::styled(
                    label,
                    Style::default().fg(Color::Gray),
                )))
            }
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_stateful_widget(list, overlay_area, &mut app.archive.list_state);
}

fn format_tokens(n: i64) -> String {
    if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

fn format_usage(u: &TaskUsage) -> String {
    format!(
        "${:.2} \u{00b7} {} in / {} out",
        u.cost_usd,
        format_tokens(u.input_tokens),
        format_tokens(u.output_tokens),
    )
}

// ── Detail-panel component functions ────────────────────────────────

fn task_detail_lines(app: &App, task: &Task) -> Vec<Line<'static>> {
    let status_color = column_color(task.status);

    // Line 1: title (bold, colored) + inline metadata (dim)
    let mut line1_spans = vec![
        Span::styled(
            task.title.clone(),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if task.sub_status != SubStatus::None {
                format!(
                    " \u{00b7} #{} \u{00b7} {} ({}) \u{00b7} {}",
                    task.id,
                    task.status.as_str(),
                    task.sub_status.as_str(),
                    task.repo_path
                )
            } else {
                format!(
                    " \u{00b7} #{} \u{00b7} {} \u{00b7} {}",
                    task.id,
                    task.status.as_str(),
                    task.repo_path
                )
            },
            Style::default().fg(MUTED),
        ),
    ];

    // Add crash/stale suffix
    if app.is_crashed(task.id) {
        line1_spans.push(Span::styled(
            " (crashed)",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    } else if app.is_stale(task.id) {
        let mins = app
            .agents
            .last_output_change
            .get(&task.id)
            .map(|t| t.elapsed().as_secs() / 60)
            .unwrap_or(0);
        line1_spans.push(Span::styled(
            format!(" (stale \u{00b7} {}m)", mins),
            Style::default().fg(Color::Yellow),
        ));
    }

    if let Some(tag) = &task.tag {
        line1_spans.push(Span::styled(
            format!(" \u{00b7} [{tag}]"),
            Style::default().fg(Color::Cyan),
        ));
    }

    if let Some(pr_url) = &task.pr_url {
        line1_spans.push(Span::styled(
            format!(" \u{00b7} PR: {pr_url}"),
            Style::default().fg(Color::Cyan),
        ));
    }

    if let Some(epic_id) = task.epic_id {
        if let Some(epic_title) = app.epic_title(epic_id) {
            line1_spans.push(Span::styled(
                format!(" \u{00b7} Epic: {epic_title} (#{epic_id})"),
                Style::default().fg(Color::Magenta),
            ));
        }
    }

    let desc_style = Style::default().fg(MUTED_LIGHT);
    let mut lines = vec![Line::from(line1_spans)];
    for desc_line in task.description.lines() {
        lines.push(Line::from(Span::styled(
            desc_line.to_string(),
            desc_style,
        )));
    }
    if task.description.is_empty() {
        lines.push(Line::from(Span::styled(String::new(), desc_style)));
    }
    if let Some(u) = app.usage.get(&task.id) {
        lines.push(Line::from(Span::styled(
            format_usage(u),
            Style::default().fg(MUTED),
        )));
    }
    lines
}

fn epic_detail_lines(app: &App, epic: &Epic) -> Vec<Line<'static>> {
    let epic_id = epic.id;
    let line1 = Line::from(vec![
        Span::styled(
            epic.title.clone(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" \u{00b7} #{} \u{00b7} {}", epic.id, epic.repo_path),
            Style::default().fg(MUTED),
        ),
    ]);
    let desc_style = Style::default().fg(MUTED_LIGHT);
    let mut lines = vec![line1];
    for desc_line in epic.description.lines() {
        lines.push(Line::from(Span::styled(
            desc_line.to_string(),
            desc_style,
        )));
    }
    if epic.description.is_empty() {
        lines.push(Line::from(Span::styled(String::new(), desc_style)));
    }
    if let Some(plan) = &epic.plan {
        lines.push(Line::from(Span::styled(
            format!("plan: {plan}"),
            Style::default().fg(MUTED),
        )));
    }

    // Subtask status list
    let mut subtasks: Vec<&Task> = app
        .tasks()
        .iter()
        .filter(|t| t.epic_id == Some(epic_id) && t.status != TaskStatus::Archived)
        .collect();
    subtasks.sort_by_key(|t| (t.status.column_index(), t.sort_order.unwrap_or(t.id.0)));

    if !subtasks.is_empty() {
        lines.push(Line::from(""));
        for task in &subtasks {
            let icon = status_icon(task.status);
            let icon_color = column_color(task.status);
            let mut spans = vec![
                Span::styled(format!("  {icon} "), Style::default().fg(icon_color)),
                Span::styled(
                    truncate_for_detail(&task.title, 40),
                    Style::default().fg(Color::Rgb(180, 184, 200)),
                ),
            ];
            if let Some(wt) = &task.worktree {
                if let Some(branch) = dispatch::branch_from_worktree(wt) {
                    spans.push(Span::styled(
                        format!(" ({branch})"),
                        Style::default().fg(Color::Rgb(86, 95, 137)),
                    ));
                }
            }
            if task.sub_status == SubStatus::Conflict {
                spans.push(Span::styled(
                    " \u{26a0} conflict",
                    Style::default().fg(Color::Red),
                ));
            }
            if let Some(pr_url) = &task.pr_url {
                spans.push(Span::styled(
                    format!(" \u{00b7} PR: {}", truncate_for_detail(pr_url, 30)),
                    Style::default().fg(Color::Cyan),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    lines
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect, _now: DateTime<Utc>) {
    // When in input mode, show the input form instead of detail
    if render_input_form(frame, app, area) {
        return;
    }

    // Top border separator
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(BORDER));

    if !app.detail_visible {
        let paragraph = Paragraph::new("").block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let lines: Vec<Line> = if let Some(task) = app.selected_task() {
        task_detail_lines(app, task)
    } else if let Some(ColumnItem::Epic(epic)) = app.selected_column_item() {
        epic_detail_lines(app, epic)
    } else {
        vec![Line::from(Span::styled(
            "No task selected",
            Style::default().fg(MUTED),
        ))]
    };

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Appends a scrollable repo-path picker list to `lines`.
fn append_repo_path_list<'a>(
    lines: &mut Vec<Line<'a>>,
    repo_paths: &'a [String],
    cursor: usize,
    height_offset: u16,
    area_height: u16,
    hint: Style,
) {
    let repo_count = repo_paths.len();
    let visible_repos = (area_height.saturating_sub(height_offset) as usize).max(1);
    let scroll = if repo_count <= visible_repos {
        0
    } else {
        cursor
            .saturating_sub(visible_repos - 1)
            .min(repo_count - visible_repos)
    };
    let cursor_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    for (i, path) in repo_paths
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_repos)
    {
        if i == cursor {
            lines.push(Line::from(vec![
                Span::styled("  ► ", cursor_style),
                Span::styled(path.as_str(), cursor_style),
            ]));
        } else {
            lines.push(Line::from(Span::styled(format!("    {path}"), hint)));
        }
    }
}

// ── Input-form component functions ──────────────────────────────────

fn input_title_lines(app: &App, active: Style, hint: Style) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            format!("  Title: {}_ ", app.input.buffer),
            active,
        )),
        Line::from(""),
        Line::from(Span::styled("  Enter to confirm, Esc to cancel", hint)),
    ]
}

fn input_tag_lines(app: &App, completed: Style, active: Style, hint: Style) -> Vec<Line<'static>> {
    let title = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(
            "  Tag: (b)ug  (f)eature  (c)hore  (e)pic  (Enter=none)",
            active,
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Select a tag or Enter to skip, Esc to cancel",
            hint,
        )),
    ]
}

fn input_description_lines(
    app: &App,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
    let title = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    let tag = app
        .input
        .task_draft
        .as_ref()
        .and_then(|d| d.tag.as_ref())
        .map(|t| t.to_string())
        .unwrap_or_else(|| "none".to_string());
    vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(format!("  Tag: {tag}"), completed)),
        Line::from(Span::styled(
            "  Description: opening $EDITOR...".to_string(),
            active,
        )),
        Line::from(""),
        Line::from(Span::styled("  Esc to cancel", hint)),
    ]
}

fn input_repo_path_lines<'a>(
    app: &'a App,
    area: Rect,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'a>> {
    let title = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    let tag = app
        .input
        .task_draft
        .as_ref()
        .and_then(|d| d.tag.as_ref())
        .map(|t| t.to_string())
        .unwrap_or_else(|| "none".to_string());
    let description = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.description.as_str())
        .unwrap_or("");
    let desc_first_line = description.lines().next().unwrap_or("");
    let desc_display = if description.contains('\n') {
        format!("{desc_first_line} ...")
    } else {
        desc_first_line.to_string()
    };
    let mut lines = vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(format!("  Tag: {tag}"), completed)),
        Line::from(Span::styled(
            format!("  Description: {desc_display}"),
            completed,
        )),
        Line::from(Span::styled(
            format!("  Repo path: {}_ ", app.input.buffer),
            active,
        )),
    ];
    if app.input.buffer.is_empty() {
        append_repo_path_list(
            &mut lines,
            &app.repo_paths,
            app.input.repo_cursor,
            7,
            area.height,
            hint,
        );
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type a path · j/k navigate · Enter select · Esc cancel",
        hint,
    )));
    lines
}

fn quick_dispatch_lines<'a>(app: &'a App, area: Rect, active: Style, hint: Style) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(Span::styled("  Quick Dispatch — select repo:", active)),
        Line::from(""),
    ];
    append_repo_path_list(
        &mut lines,
        &app.repo_paths,
        app.input.repo_cursor,
        6,
        area.height,
        hint,
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  j/k navigate · Enter select · 1-9 shortcut · Esc cancel",
        hint,
    )));
    lines
}

fn confirm_retry_lines(app: &App, id: TaskId) -> Vec<Line<'static>> {
    let label = if app.is_crashed(id) {
        "crashed"
    } else {
        "stale"
    };
    let warning = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    let hint = Style::default().fg(Color::DarkGray);
    vec![
        Line::from(Span::styled(
            format!("  Agent is {label}. What do you want to do?"),
            warning,
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  [r] Resume (--continue in existing worktree)",
            hint,
        )),
        Line::from(Span::styled(
            "  [f] Fresh start (clean worktree + new dispatch)",
            hint,
        )),
        Line::from(Span::styled("  [Esc] Cancel", hint)),
    ]
}

fn input_epic_title_lines(app: &App, active: Style, hint: Style) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            format!("  Title: {}_ ", app.input.buffer),
            active,
        )),
        Line::from(""),
        Line::from(Span::styled("  Enter to confirm, Esc to cancel", hint)),
    ]
}

fn input_epic_description_lines(
    app: &App,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
    let title = app
        .input
        .epic_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(
            "  Description: opening $EDITOR...".to_string(),
            active,
        )),
        Line::from(""),
        Line::from(Span::styled("  Esc to cancel", hint)),
    ]
}

fn input_epic_repo_path_lines<'a>(
    app: &'a App,
    area: Rect,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'a>> {
    let title = app
        .input
        .epic_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    let description = app
        .input
        .epic_draft
        .as_ref()
        .map(|d| d.description.as_str())
        .unwrap_or("");
    let desc_first_line = description.lines().next().unwrap_or("");
    let desc_display = if description.contains('\n') {
        format!("{desc_first_line} ...")
    } else {
        desc_first_line.to_string()
    };
    let mut lines = vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(
            format!("  Description: {desc_display}"),
            completed,
        )),
        Line::from(Span::styled(
            format!("  Repo path: {}_ ", app.input.buffer),
            active,
        )),
    ];
    if app.input.buffer.is_empty() {
        append_repo_path_list(
            &mut lines,
            &app.repo_paths,
            app.input.repo_cursor,
            7,
            area.height,
            hint,
        );
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type a path · j/k navigate · Enter select · Esc cancel",
        hint,
    )));
    lines
}

fn render_input_form(frame: &mut Frame, app: &App, area: Rect) -> bool {
    let completed = Style::default().fg(Color::White);
    let active = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let hint = Style::default().fg(Color::DarkGray);

    let lines: Vec<Line> = match &app.input.mode {
        InputMode::InputTitle => input_title_lines(app, active, hint),
        InputMode::InputTag => input_tag_lines(app, completed, active, hint),
        InputMode::InputDescription => input_description_lines(app, completed, active, hint),
        InputMode::InputRepoPath => input_repo_path_lines(app, area, completed, active, hint),
        InputMode::QuickDispatch => quick_dispatch_lines(app, area, active, hint),
        InputMode::ConfirmRetry(id) => confirm_retry_lines(app, *id),
        InputMode::InputEpicTitle => input_epic_title_lines(app, active, hint),
        InputMode::InputEpicDescription => {
            input_epic_description_lines(app, completed, active, hint)
        }
        InputMode::InputEpicRepoPath => {
            input_epic_repo_path_lines(app, area, completed, active, hint)
        }
        _ => return false,
    };

    let is_epic_input = matches!(
        app.input.mode,
        InputMode::InputEpicTitle | InputMode::InputEpicDescription | InputMode::InputEpicRepoPath
    );

    let block_title = match &app.input.mode {
        InputMode::QuickDispatch => " Quick Dispatch ",
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

fn render_error_popup(frame: &mut Frame, app: &App, area: Rect) {
    let Some(error_msg) = &app.error_popup else {
        return;
    };

    let popup_width = (area.width * 60 / 100).clamp(30, 60);
    let popup_height = 7_u16;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Error ")
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(Color::Red))
        .title_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            error_msg.as_str(),
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to dismiss",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, popup_area);
}

fn render_help_overlay(frame: &mut Frame, app: &App, area: Rect) {
    if app.input.mode != InputMode::Help {
        return;
    }

    let popup_width = (area.width * 80 / 100).clamp(40, 72);
    let popup_height = (area.height * 80 / 100).clamp(25, 36);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let header = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc = Style::default().fg(Color::Gray);
    let note = Style::default().fg(Color::DarkGray);

    let lines = if matches!(app.view_mode(), ViewMode::ReviewBoard { .. }) {
        vec![
            Line::from(""),
            Line::from(Span::styled("  Review Board", header)),
            Line::from(vec![
                Span::styled("  h/\u{2190}", key),
                Span::styled(" previous column   ", desc),
                Span::styled("l/\u{2192}", key),
                Span::styled(" next column", desc),
            ]),
            Line::from(vec![
                Span::styled("  j/\u{2193}", key),
                Span::styled(" next PR           ", desc),
                Span::styled("k/\u{2191}", key),
                Span::styled(" previous PR", desc),
            ]),
            Line::from(vec![
                Span::styled("  Enter", key),
                Span::styled(" detail panel  ", desc),
                Span::styled("p", key),
                Span::styled(" open PR in browser", desc),
            ]),
            Line::from(""),
            Line::from(Span::styled("  Actions", header)),
            Line::from(vec![
                Span::styled("  r", key),
                Span::styled(" refresh from GitHub  ", desc),
                Span::styled("d", key),
                Span::styled(" dispatch review agent", desc),
            ]),
            Line::from(vec![
                Span::styled("  f", key),
                Span::styled(" filter repos", desc),
            ]),
            Line::from(""),
            Line::from(Span::styled("  General", header)),
            Line::from(vec![
                Span::styled("  Shift+Tab", key),
                Span::styled(" toggle Reviews / My PRs", desc),
            ]),
            Line::from(vec![
                Span::styled("  Tab/Esc", key),
                Span::styled(" back to Task Board  ", desc),
                Span::styled("q", key),
                Span::styled(" quit", desc),
            ]),
            Line::from(vec![
                Span::styled("  ?", key),
                Span::styled(" close this help", desc),
            ]),
            Line::from(""),
            Line::from(Span::styled("  Press ? or Esc to close", note)),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled("  Navigation", header)),
            Line::from(vec![
                Span::styled("  h/\u{2190}", key),
                Span::styled(" previous column   ", desc),
                Span::styled("j/\u{2193}", key),
                Span::styled(" next task", desc),
            ]),
            Line::from(vec![
                Span::styled("  l/\u{2192}", key),
                Span::styled(" next column       ", desc),
                Span::styled("k/\u{2191}", key),
                Span::styled(" previous task", desc),
            ]),
            Line::from(vec![
                Span::styled("  Enter", key),
                Span::styled(" detail panel       ", desc),
                Span::styled("e", key),
                Span::styled(" edit / enter epic", desc),
            ]),
            Line::from(vec![
                Span::styled("  q", key),
                Span::styled(" exit epic (in epic view)   ", desc),
                Span::styled("Esc", key),
                Span::styled(" clear selection", desc),
            ]),
            Line::from(""),
            Line::from(Span::styled("  Actions", header)),
            Line::from(vec![
                Span::styled("  n", key),
                Span::styled(" new task   ", desc),
                Span::styled("E", key),
                Span::styled(" new epic   ", desc),
                Span::styled("N", key),
                Span::styled(" notifications", desc),
            ]),
            Line::from(vec![
                Span::styled("  d", key),
                Span::styled(" dispatch*  ", desc),
                Span::styled("m", key),
                Span::styled(" move fwd   ", desc),
                Span::styled("M", key),
                Span::styled(" move back", desc),
            ]),
            Line::from(vec![
                Span::styled("  x", key),
                Span::styled(" archive    ", desc),
                Span::styled("D", key),
                Span::styled(" quick dsp  ", desc),
                Span::styled("g", key),
                Span::styled(" session/board", desc),
            ]),
            Line::from(vec![
                Span::styled("  G", key),
                Span::styled(" session    ", desc),
                Span::styled("(epic: jump to subtask tmux)", note),
            ]),
            Line::from(vec![
                Span::styled("  H", key),
                Span::styled(" history    ", desc),
                Span::styled("V", key),
                Span::styled(" epic done  ", desc),
                Span::styled("a", key),
                Span::styled(" select all", desc),
            ]),
            Line::from(vec![
                Span::styled("  Space", key),
                Span::styled(" select  ", desc),
                Span::styled("f", key),
                Span::styled(" filter repos  ", desc),
                Span::styled("W", key),
                Span::styled(" wrap up    ", desc),
                Span::styled("(task: rebase/PR, epic: batch)", note),
            ]),
            Line::from(vec![
                Span::styled("  T", key),
                Span::styled(" detach tmux panel  ", desc),
                Span::styled("(Review tasks, supports batch)", note),
            ]),
            Line::from(vec![
                Span::styled("  P", key),
                Span::styled(" merge PR  ", desc),
                Span::styled("p", key),
                Span::styled(" open PR in browser", desc),
            ]),
            Line::from(vec![
                Span::styled("  J/K", key),
                Span::styled(" reorder item up/down in column", desc),
            ]),
            Line::from(""),
            Line::from(Span::styled("  * d is context-dependent:", note)),
            Line::from(Span::styled(
                "    Backlog (no plan) \u{2192} brainstorm",
                note,
            )),
            Line::from(Span::styled(
                "    Backlog (has plan) \u{2192} dispatch",
                note,
            )),
            Line::from(Span::styled(
                "    Running \u{2192} resume (if window gone)",
                note,
            )),
            Line::from(Span::styled(
                "    Epic \u{2192} dispatch next backlog subtask",
                note,
            )),
            Line::from(""),
            Line::from(Span::styled("  General", header)),
            Line::from(vec![
                Span::styled("  ?", key),
                Span::styled(" this help  ", desc),
                Span::styled("N", key),
                Span::styled(" notify on/off  ", desc),
                Span::styled("q", key),
                Span::styled(" quit (or exit epic)", desc),
            ]),
            Line::from(vec![
                Span::styled("  Tab", key),
                Span::styled(" switch to Review Board", desc),
            ]),
            Line::from(""),
            Line::from(Span::styled("  Press ? or Esc to close", note)),
        ]
    };

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup_area);
}

fn render_repo_filter_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let is_filter_mode = matches!(
        app.mode(),
        InputMode::RepoFilter | InputMode::InputPresetName | InputMode::ConfirmDeletePreset
    );
    if !is_filter_mode {
        return;
    }

    let repo_count = app.repo_paths().len();
    let preset_count = app.filter_presets().len();
    let preset_lines = if preset_count > 0 {
        preset_count + 2
    } else {
        0
    }; // header + presets + blank line
    let input_line = if matches!(app.mode(), InputMode::InputPresetName) {
        1
    } else {
        0
    };
    // Cap popup height to screen minus 4; repos may scroll if they don't fit
    // +6: blank(1) + preset_lines + blank(1) + 2_help_lines(2) + borders(2)
    let popup_height = (repo_count as u16 + preset_lines as u16 + input_line as u16 + 6)
        .clamp(7, area.height.saturating_sub(4));
    let popup_width = (area.width * 70 / 100).clamp(30, 60);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    // How many repo rows fit: content height minus non-repo fixed lines
    // non_repo = blank(1) + preset_lines + blank(1) + 2_help_lines(2) = 4 + preset_lines
    let content_height = popup_height.saturating_sub(2) as usize; // minus borders
    let non_repo_lines = preset_lines + input_line + 4; // blank + presets + blank + 2 help lines
    let visible_repos = content_height.saturating_sub(non_repo_lines).max(1);

    let cursor = app.input.repo_cursor;
    let scroll = if repo_count <= visible_repos {
        0
    } else {
        cursor
            .saturating_sub(visible_repos - 1)
            .min(repo_count - visible_repos)
    };

    frame.render_widget(Clear, popup_area);

    let mode_label = match app.repo_filter_mode() {
        RepoFilterMode::Include => "include",
        RepoFilterMode::Exclude => "exclude",
    };
    let block = Block::default()
        .title(format!(" Repo Filter ({mode_label}) "))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let key_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::Gray);
    let note_style = Style::default().fg(Color::DarkGray);
    let cursor_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![Line::from("")];

    // Presets section
    if !app.filter_presets().is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  Presets:",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )]));
        for (i, (name, _, mode)) in app.filter_presets().iter().enumerate() {
            let letter = (b'A' + i as u8) as char;
            let mode_tag = match mode {
                RepoFilterMode::Include => "",
                RepoFilterMode::Exclude => " (excl)",
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {letter}"), key_style),
                Span::styled(format!(". {name}{mode_tag}"), desc_style),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Repo list (scrollable)
    if scroll > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ↑ {} more", scroll),
            note_style,
        )));
    }
    for (i, path) in app
        .repo_paths()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_repos)
    {
        let checked = if app.repo_filter().contains(path) {
            "x"
        } else {
            " "
        };
        if i == cursor {
            lines.push(Line::from(vec![
                Span::styled("  ►", cursor_style),
                Span::styled(format!(" [{checked}] {path}"), cursor_style),
            ]));
        } else {
            let num = i + 1;
            lines.push(Line::from(vec![
                Span::styled(format!("  {num}"), key_style),
                Span::styled(format!(". [{checked}] {path}"), desc_style),
            ]));
        }
    }
    let remaining = repo_count.saturating_sub(scroll + visible_repos);
    if remaining > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ↓ {} more", remaining),
            note_style,
        )));
    }

    lines.push(Line::from(""));

    // Input line for preset name
    if matches!(app.mode(), InputMode::InputPresetName) {
        lines.push(Line::from(vec![
            Span::styled("  Name: ", key_style),
            Span::styled(app.input_buffer(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(Color::DarkGray)),
        ]));
    }

    // Help text
    let all_selected = app.repo_filter().len() == app.repo_paths().len();
    let a_label = if all_selected {
        "clear all"
    } else {
        "select all"
    };
    match app.mode() {
        InputMode::InputPresetName => {
            lines.push(Line::from(vec![
                Span::styled("  Enter", key_style),
                Span::styled(": save  ", note_style),
                Span::styled("Esc", key_style),
                Span::styled(": cancel", note_style),
            ]));
        }
        InputMode::ConfirmDeletePreset => {
            lines.push(Line::from(vec![
                Span::styled("  A-Z", key_style),
                Span::styled(": delete preset  ", note_style),
                Span::styled("Esc", key_style),
                Span::styled(": cancel", note_style),
            ]));
        }
        _ => {
            lines.push(Line::from(vec![
                Span::styled("  j/k", key_style),
                Span::styled(": navigate  ", note_style),
                Span::styled("Space", key_style),
                Span::styled(": toggle  ", note_style),
                Span::styled("a", key_style),
                Span::styled(format!(": {a_label}  "), note_style),
                Span::styled("Tab", key_style),
                Span::styled(": incl/excl", note_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  s", key_style),
                Span::styled(": save preset  ", note_style),
                Span::styled("x", key_style),
                Span::styled(": del preset  ", note_style),
                Span::styled("q/Enter/Esc", key_style),
                Span::styled(": close", note_style),
            ]));
        }
    }

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup_area);
}

fn render_review_repo_filter_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let repos = app.active_review_repos();
    let repo_count = repos.len();
    let popup_height = (repo_count as u16 + 5).clamp(7, area.height.saturating_sub(4));
    let popup_width = (area.width * 70 / 100).clamp(30, 60);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let review_mode_label = match app.review_repo_filter_mode() {
        RepoFilterMode::Include => "include",
        RepoFilterMode::Exclude => "exclude",
    };
    let block = Block::default()
        .title(format!(" Review Repo Filter ({review_mode_label}) "))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let key_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::Gray);
    let note_style = Style::default().fg(Color::DarkGray);

    let mut lines = vec![Line::from("")];

    for (i, repo) in repos.iter().enumerate() {
        let num = i + 1;
        let checked = if app.review_repo_filter().contains(repo) {
            "x"
        } else {
            " "
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {num}"), key_style),
            Span::styled(format!(". [{checked}] {repo}"), desc_style),
        ]));
    }

    lines.push(Line::from(""));

    let all_selected = !repos.is_empty() && app.review_repo_filter().len() == repos.len();
    let a_label = if all_selected {
        "clear all"
    } else {
        "select all"
    };
    lines.push(Line::from(vec![
        Span::styled("  a", key_style),
        Span::styled(format!(": {a_label}  "), note_style),
        Span::styled("Tab", key_style),
        Span::styled(": incl/excl  ", note_style),
        Span::styled("Enter/Esc", key_style),
        Span::styled(": close", note_style),
    ]));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup_area);
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(msg) = &app.status_message {
        let bar = Paragraph::new(msg.as_str()).style(Style::default().fg(Color::Yellow));
        frame.render_widget(bar, area);
        return;
    }

    // Archive mode status bar
    if app.show_archived() {
        let key_color = MUTED;
        let label_style = Style::default().fg(MUTED);
        let spans = vec![
            Span::styled(
                "x",
                Style::default().fg(key_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" delete  ", label_style),
            Span::styled(
                "e",
                Style::default().fg(key_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" edit  ", label_style),
            Span::styled(
                "H",
                Style::default().fg(key_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" close  ", label_style),
            Span::styled(
                "q",
                Style::default().fg(key_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit  ", label_style),
        ];
        let bar = Paragraph::new(Line::from(spans));
        frame.render_widget(bar, area);
        return;
    }

    match &app.input.mode {
        InputMode::Normal => {
            let key_color = TaskStatus::from_column_index(app.selected_column())
                .map(column_color)
                .unwrap_or(MUTED);
            let spans = if app.has_selection() {
                let count = app.selected_tasks().len() + app.selected_epics().len();
                let has_tasks = !app.selected_tasks().is_empty();
                batch_action_hints(count, key_color, has_tasks)
            } else if let Some(ColumnItem::Epic(epic)) = app.selected_column_item() {
                epic_action_hints(epic, key_color)
            } else {
                action_hints(app.selected_task(), key_color)
            };
            let bar = Paragraph::new(Line::from(spans));
            frame.render_widget(bar, area);
        }
        InputMode::InputTitle => {
            let bar = Paragraph::new("Creating task: enter title")
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::InputDescription => {
            let bar = Paragraph::new("Creating task: opening $EDITOR for description")
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::InputRepoPath => {
            let bar = Paragraph::new("Creating task: enter repo path")
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::InputTag => {
            let text = app
                .status_message
                .as_deref()
                .unwrap_or("Tag: (b)ug (f)eature (c)hore (e)pic (Enter=none)");
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmDelete => {
            let text = app.status_message.as_deref().unwrap_or("Delete? (y/n)");
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Red));
            frame.render_widget(bar, area);
        }
        InputMode::QuickDispatch => {
            let bar = Paragraph::new("Quick dispatch: select repo path")
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmRetry(_) => {
            let bar = Paragraph::new("[r] Resume  [f] Fresh start  [Esc] Cancel")
                .style(Style::default().fg(Color::Red));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmArchive => {
            let bar =
                Paragraph::new("Archive task? (y/n)").style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmDone(_) => {
            let text = app
                .status_message
                .as_deref()
                .unwrap_or("Move to Done? (y/n)");
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::InputEpicTitle => {
            let bar = Paragraph::new("Creating epic: enter title")
                .style(Style::default().fg(Color::Magenta));
            frame.render_widget(bar, area);
        }
        InputMode::InputEpicDescription => {
            let bar = Paragraph::new("Creating epic: opening $EDITOR for description")
                .style(Style::default().fg(Color::Magenta));
            frame.render_widget(bar, area);
        }
        InputMode::InputEpicRepoPath => {
            let bar = Paragraph::new("Creating epic: enter repo path")
                .style(Style::default().fg(Color::Magenta));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmDeleteEpic => {
            let text = app
                .status_message
                .as_deref()
                .unwrap_or("Delete epic and subtasks? (y/n)");
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Red));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmArchiveEpic => {
            let bar = Paragraph::new("Archive epic and subtasks? (y/n)")
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::Help => {
            let bar = Paragraph::new("Press ? or Esc to close help")
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(bar, area);
        }
        InputMode::RepoFilter => {
            let bar = Paragraph::new("Filter repos: 1-9 toggle, (a)ll, q/Enter/Esc close")
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmMergePr(_) => {
            let text = app
                .status_message
                .as_deref()
                .unwrap_or("Merge PR? (y/n)");
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Green));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmWrapUp(_) => {
            let text = app
                .status_message
                .as_deref()
                .unwrap_or("Wrap up: (r) rebase  (p) create PR  (Esc) cancel");
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::InputPresetName => {
            let bar = Paragraph::new("Enter preset name, Enter to save, Esc to cancel")
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmDeletePreset => {
            let bar = Paragraph::new("Press A-Z to delete preset, Esc to cancel")
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmEpicWrapUp(_) => {
            let text = app
                .status_message
                .as_deref()
                .unwrap_or("Epic wrap up: (r) rebase all  (p) PR all  (Esc) cancel");
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmDetachTmux(_) => {
            let text = app
                .status_message
                .as_deref()
                .unwrap_or("Detach tmux panel? (y/n)");
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ReviewRepoFilter => {
            let bar = Paragraph::new("Filter repos: 1-9 toggle, (a)ll, Enter/Esc close")
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(bar, area);
        }
        InputMode::SecurityRepoFilter => {
            let bar = Paragraph::new("Filter repos: 1-9 toggle, (a)ll, Enter/Esc close")
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmEditTask(_) => {
            let text = app.status_message.as_deref().unwrap_or("Edit task? (y/n)");
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmBatchApprove(ref urls) => {
            let text = format!("Approve {} PRs? (y/n)", urls.len());
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmBatchMerge(ref urls) => {
            let text = format!("Merge {} PRs? (y/n)", urls.len());
            let bar = Paragraph::new(text).style(Style::default().fg(Color::Green));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmQuit => {
            let bar =
                Paragraph::new("Quit dispatch? (y/n)").style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
    }
}

/// Build context-sensitive keybinding hint spans for the status bar.
/// Returns styled spans showing available actions for the selected task.
pub(in crate::tui) fn action_hints(task: Option<&Task>, key_color: Color) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(MUTED);

    let mut spans: Vec<Span<'static>> = Vec::new();

    let mut push_hint = |key: &'static str, label: &'static str| {
        spans.push(Span::styled(
            key,
            Style::default().fg(key_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}  "), label_style));
    };

    if let Some(task) = task {
        match task.status {
            TaskStatus::Backlog => {
                let d_label = if task.plan.is_some() {
                    "dispatch"
                } else {
                    "brainstorm"
                };
                push_hint("d", d_label);
                push_hint("e", "edit");
                push_hint("m", "move");
                push_hint("x", "archive");
            }
            TaskStatus::Running => {
                if task.tmux_window.is_some() {
                    push_hint("g", "session");
                } else if task.worktree.is_some() {
                    push_hint("d", "resume");
                }
                push_hint("e", "edit");
                push_hint("m", "move");
                push_hint("M", "back");
                push_hint("x", "archive");
            }
            TaskStatus::Review => {
                if task.worktree.is_some() {
                    push_hint("W", "wrap up");
                }
                if task.tmux_window.is_some() {
                    push_hint("g", "session");
                    push_hint("T", "detach");
                } else if task.worktree.is_some() {
                    push_hint("d", "resume");
                }
                push_hint("e", "edit");
                push_hint("m", "move");
                push_hint("M", "back");
                push_hint("x", "archive");
            }
            TaskStatus::Done => {
                push_hint("e", "edit");
                push_hint("M", "back");
                push_hint("x", "archive");
            }
            TaskStatus::Archived => {}
        }
        if task.pr_url.is_some() {
            push_hint("p", "open PR");
            if task.sub_status == SubStatus::Approved {
                push_hint("P", "merge");
            }
        }
    }

    push_hint("^g", "back");
    push_hint("a", "select all");
    push_hint("n", "new");
    push_hint("E", "epic");
    push_hint("D", "quick");
    push_hint("H", "history");
    push_hint("q", "quit");

    spans
}

/// Build context-sensitive keybinding hints for a selected epic.
pub(in crate::tui) fn epic_action_hints(epic: &Epic, key_color: Color) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(MUTED);

    let mut spans: Vec<Span<'static>> = Vec::new();

    let mut push_hint = |key: &'static str, label: &'static str| {
        spans.push(Span::styled(
            key,
            Style::default().fg(key_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}  "), label_style));
    };

    if epic.plan.is_some() {
        push_hint("d", "dispatch");
    } else {
        push_hint("d", "plan");
    }
    push_hint("g", "board");
    push_hint("G", "session");
    push_hint("Enter", "detail");
    push_hint("e", "edit");
    push_hint("W", "wrap up");
    push_hint("m", "status \u{2192}");
    push_hint("M", "status \u{2190}");
    push_hint("x", "archive");

    push_hint("a", "select all");
    push_hint("n", "new");
    push_hint("E", "epic");
    push_hint("D", "quick");
    push_hint("H", "history");
    push_hint("q", "quit");

    spans
}

/// Build status bar hints when tasks are batch-selected.
fn batch_action_hints(count: usize, key_color: Color, has_tasks: bool) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(MUTED);
    let count_style = Style::default().fg(YELLOW).add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(format!("{count} selected  "), count_style));

    let mut push_hint = |key: &'static str, label: &'static str| {
        spans.push(Span::styled(
            key,
            Style::default().fg(key_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}  "), label_style));
    };

    if has_tasks {
        push_hint("m", "move");
        push_hint("M", "back");
    }
    push_hint("x", "archive");
    push_hint("a", "select all");
    push_hint("Space", "toggle");
    push_hint("Esc", "clear");
    spans
}

// ---------------------------------------------------------------------------
// Review board rendering
// ---------------------------------------------------------------------------

fn review_action_hints(has_pr: bool, is_author_mode: bool) -> Vec<Span<'static>> {
    let key_color = Color::Cyan;
    let label_style = Style::default().fg(MUTED);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut push_hint = |key: &'static str, label: &'static str| {
        spans.push(Span::styled(
            key,
            Style::default().fg(key_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}  "), label_style));
    };
    if has_pr {
        push_hint("Enter", "open PR");
    }
    push_hint("r", "refresh");
    if is_author_mode {
        push_hint("D", "dispatch filter");
    }
    push_hint("Tab", "task board");
    push_hint("?", "help");
    push_hint("q", "quit");
    spans
}

fn bot_action_hints(has_pr: bool) -> Vec<Span<'static>> {
    let key_color = Color::Cyan;
    let label_style = Style::default().fg(MUTED);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut push_hint = |key: &'static str, label: &'static str| {
        spans.push(Span::styled(
            key,
            Style::default().fg(key_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}  "), label_style));
    };
    push_hint("Space", "select");
    push_hint("a", "select all");
    if has_pr {
        push_hint("d", "review");
        push_hint("p", "open");
    }
    push_hint("r", "refresh");
    push_hint("Tab", "task board");
    spans
}

fn review_column_color(decision: ReviewDecision) -> Color {
    match decision {
        ReviewDecision::ReviewRequired => CYAN,
        ReviewDecision::WaitingForResponse => YELLOW,
        ReviewDecision::ChangesRequested => RED_DIM,
        ReviewDecision::Approved => GREEN,
    }
}

fn review_cursor_bg_color(decision: ReviewDecision) -> Color {
    match decision {
        ReviewDecision::ReviewRequired => Color::Rgb(24, 48, 52),
        ReviewDecision::WaitingForResponse => Color::Rgb(52, 44, 20),
        ReviewDecision::ChangesRequested => Color::Rgb(56, 32, 32),
        ReviewDecision::Approved => Color::Rgb(32, 52, 36),
    }
}

fn review_column_bg_color(decision: ReviewDecision) -> Color {
    match decision {
        ReviewDecision::ReviewRequired => Color::Rgb(26, 36, 38),
        ReviewDecision::WaitingForResponse => Color::Rgb(36, 34, 26),
        ReviewDecision::ChangesRequested => Color::Rgb(36, 28, 28),
        ReviewDecision::Approved => Color::Rgb(27, 36, 30),
    }
}

/// Render the review board view.
pub fn render_review_board(frame: &mut Frame, app: &mut App, area: Rect) {
    let detail_height = if app.review_detail_visible() { 8 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),             // tab bar
            Constraint::Length(1),             // summary row
            Constraint::Min(1),                // board
            Constraint::Length(detail_height), // detail panel
            Constraint::Length(1),             // status bar
        ])
        .split(area);

    render_tab_bar(frame, app, chunks[0]);
    render_review_summary_row(frame, app, chunks[1]);

    let filtered = app.active_review_prs();
    if filtered.is_empty() {
        let is_loading = match app.view_mode() {
            ViewMode::ReviewBoard {
                mode: ReviewBoardMode::Author,
                ..
            } => app.my_prs_loading(),
            ViewMode::ReviewBoard {
                mode: ReviewBoardMode::Dependabot,
                ..
            } => app.bot_prs_loading(),
            _ => app.review_board_loading(),
        };
        let is_empty = match app.view_mode() {
            ViewMode::ReviewBoard {
                mode: ReviewBoardMode::Author,
                ..
            } => app.my_prs().is_empty(),
            ViewMode::ReviewBoard {
                mode: ReviewBoardMode::Dependabot,
                ..
            } => app.bot_prs().is_empty(),
            _ => app.review_prs().is_empty(),
        };
        let msg = if is_loading {
            "Loading..."
        } else if is_empty {
            "No PRs found. Press r to refresh."
        } else {
            "All PRs filtered out."
        };
        let p = Paragraph::new(msg)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, chunks[2]);
    } else {
        render_review_columns(frame, app, chunks[2]);
    }

    render_review_detail(frame, app, chunks[3]);

    // Status bar: transient message takes priority; fall back to persistent error
    if let Some(msg) = app.status_message() {
        let status = Paragraph::new(msg.to_string()).style(Style::default().fg(Color::Yellow));
        frame.render_widget(status, chunks[4]);
    } else if let Some(err) = app.last_review_error() {
        let status = Paragraph::new(format!("Error: {err}")).style(Style::default().fg(Color::Red));
        frame.render_widget(status, chunks[4]);
    } else if app.has_bot_pr_selection() {
        let count = app.selected_bot_prs().len();
        let text = format!("{count} selected  [A] approve  [m] merge  [Esc] clear");
        let status = Paragraph::new(text).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(status, chunks[4]);
    } else {
        let has_pr = app.selected_review_pr().is_some();
        let is_author_mode = matches!(
            app.view_mode(),
            ViewMode::ReviewBoard {
                mode: ReviewBoardMode::Author,
                ..
            }
        );
        let is_bot_mode = matches!(
            app.view_mode(),
            ViewMode::ReviewBoard {
                mode: ReviewBoardMode::Dependabot,
                ..
            }
        );
        if is_bot_mode {
            let hints = Paragraph::new(Line::from(bot_action_hints(has_pr)));
            frame.render_widget(hints, chunks[4]);
        } else {
            let hints = Paragraph::new(Line::from(review_action_hints(has_pr, is_author_mode)));
            frame.render_widget(hints, chunks[4]);
        }
    }

    // Filter overlay (on top of everything)
    if matches!(app.mode(), InputMode::ReviewRepoFilter) {
        render_review_repo_filter_overlay(frame, app, area);
    }
}

fn render_review_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(BORDER));

    if !app.review_detail_visible() {
        let paragraph = Paragraph::new("").block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let Some(pr) = app.selected_review_pr() else {
        let paragraph = Paragraph::new("").block(block);
        frame.render_widget(paragraph, area);
        return;
    };

    let decision_color = review_column_color(pr.review_decision);
    let now = Utc::now();
    let age = format_age(pr.created_at, now);

    // Line 1: title + CI status
    let ci_color = match pr.ci_status {
        CiStatus::Success => Color::Green,
        CiStatus::Failure => Color::Red,
        CiStatus::Pending => Color::Yellow,
        CiStatus::None => Color::DarkGray,
    };
    let ci_label = match pr.ci_status {
        CiStatus::Success => "passing",
        CiStatus::Failure => "failing",
        CiStatus::Pending => "pending",
        CiStatus::None => "no checks",
    };
    let line1 = Line::from(vec![
        Span::styled(
            format!("{}#{} {}", pr.repo, pr.number, pr.title),
            Style::default()
                .fg(decision_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  CI: {} {ci_label}", pr.ci_status.symbol()),
            Style::default().fg(ci_color),
        ),
    ]);

    // Line 2: metadata
    let line2 = Line::from(Span::styled(
        format!(
            "@{} \u{00b7} {} \u{00b7} +{}/-{}",
            pr.author, age, pr.additions, pr.deletions
        ),
        Style::default().fg(MUTED),
    ));

    // Line 3: reviewer list
    let reviewer_spans: Vec<String> = pr
        .reviewers
        .iter()
        .map(|r| {
            let icon = match r.decision {
                Some(ReviewDecision::Approved) => "\u{2713}",
                Some(ReviewDecision::ChangesRequested) => "\u{2717}",
                _ => "\u{23f3}",
            };
            format!("@{} {icon}", r.login)
        })
        .collect();
    let reviewer_line = if reviewer_spans.is_empty() {
        "No reviewers".to_string()
    } else {
        format!("Reviews: {}", reviewer_spans.join(" \u{00b7} "))
    };
    let line3 = Line::from(Span::styled(
        reviewer_line,
        Style::default().fg(MUTED_LIGHT),
    ));

    // Lines 4+: PR body (truncated to fit remaining space)
    let body_lines: Vec<Line> = pr
        .body
        .lines()
        .take(5)
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(Color::DarkGray),
            ))
        })
        .collect();

    let mut lines = vec![line1, line2, line3];
    lines.extend(body_lines);

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn render_review_summary_row(frame: &mut Frame, app: &App, area: Rect) {
    let sel = app.review_selection();
    let selected_col = sel.map(|s| s.column()).unwrap_or(0);
    let filtered = app.active_review_prs();
    let mode = match app.view_mode() {
        ViewMode::ReviewBoard { mode, .. } => *mode,
        _ => ReviewBoardMode::Reviewer,
    };
    let col_count = mode.column_count();

    let segments = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, col_count as u32); col_count])
        .split(area);

    for i in 0..col_count {
        let count = filtered.iter().filter(|pr| mode.pr_column(pr) == i).count();
        let is_focused = i == selected_col;
        let prefix = if is_focused { "\u{25b8} " } else { "\u{25e6} " };
        let label = format!("{prefix}{} ({count})", mode.column_label(i));

        // Map column index to ReviewDecision for coloring
        let decision_for_color = if matches!(mode, ReviewBoardMode::Dependabot) {
            match i {
                0 => ReviewDecision::Approved,
                1 => ReviewDecision::ChangesRequested,
                2 => ReviewDecision::ReviewRequired,
                3 => ReviewDecision::Approved,
                _ => ReviewDecision::ReviewRequired,
            }
        } else {
            ReviewDecision::from_column_index(i).unwrap_or(ReviewDecision::ReviewRequired)
        };

        let color = review_column_color(decision_for_color);
        let style = if is_focused {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let p = Paragraph::new(label).style(style);
        frame.render_widget(p, segments[i]);
    }
}

fn render_review_columns(frame: &mut Frame, app: &mut App, area: Rect) {
    let sel_col = app.review_selection().map(|s| s.column()).unwrap_or(0);
    let dispatch_urls = app.dispatch_pr_urls();
    let mode = match app.view_mode() {
        ViewMode::ReviewBoard { mode, .. } => *mode,
        _ => ReviewBoardMode::Reviewer,
    };
    let col_count = mode.column_count();
    let is_bot_mode = matches!(mode, ReviewBoardMode::Dependabot);

    let col_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, col_count as u32); col_count])
        .split(area);

    for i in 0..col_count {
        let is_focused = i == sel_col;
        let prs: Vec<&ReviewPr> = app.active_prs_for_column(i);

        let selected_row = app.review_selection().map(|s| s.row(i)).unwrap_or(0);
        let items: Vec<ListItem> = prs
            .iter()
            .enumerate()
            .map(|(row, pr)| {
                let is_dispatch = dispatch_urls.contains(pr.url.as_str());
                let is_selected = is_bot_mode && app.selected_bot_prs().contains(&pr.url);
                build_review_pr_item(
                    pr,
                    mode,
                    i,
                    is_focused && row == selected_row,
                    is_dispatch,
                    is_selected,
                )
            })
            .collect();

        // Use ReviewDecision column color for Reviewer/Author, CI-based for Dependabot
        let decision_for_color = if is_bot_mode {
            match i {
                0 => ReviewDecision::Approved,         // CI Passing → green-ish
                1 => ReviewDecision::ChangesRequested, // CI Failing → red-ish
                2 => ReviewDecision::ReviewRequired,   // CI Pending → yellow-ish
                3 => ReviewDecision::Approved,
                _ => ReviewDecision::ReviewRequired,
            }
        } else {
            ReviewDecision::from_column_index(i).unwrap_or(ReviewDecision::ReviewRequired)
        };

        let bg = if is_focused {
            review_column_bg_color(decision_for_color)
        } else {
            Color::Reset
        };

        let list = List::new(items).block(Block::default().style(Style::default().bg(bg)));

        let mut list_state = ListState::default();
        if is_focused {
            list_state.select(Some(selected_row));
        }

        frame.render_stateful_widget(list, col_areas[i], &mut list_state);

        // Write back the list state for scroll tracking
        if let Some(sel) = app.review_selection_mut() {
            sel.list_states[i] = list_state;
        }
    }
}

fn build_review_pr_item(
    pr: &ReviewPr,
    mode: ReviewBoardMode,
    col: usize,
    is_cursor: bool,
    is_dispatch: bool,
    is_selected: bool,
) -> ListItem<'static> {
    // Map to a ReviewDecision for color purposes
    let decision_for_color = if matches!(mode, ReviewBoardMode::Dependabot) {
        match col {
            0 => ReviewDecision::Approved,
            1 => ReviewDecision::ChangesRequested,
            2 => ReviewDecision::ReviewRequired,
            3 => ReviewDecision::Approved,
            _ => ReviewDecision::ReviewRequired,
        }
    } else {
        ReviewDecision::from_column_index(col).unwrap_or(ReviewDecision::ReviewRequired)
    };
    let color = review_column_color(decision_for_color);
    let now = Utc::now();
    let age = format_age(pr.created_at, now);

    // Line 1: selection marker + stripe + dispatch badge + repo#number + title
    let select_prefix = if is_selected { "* " } else { "" };
    let stripe = if is_cursor { "\u{258c} " } else { "\u{258e} " };
    let dispatch_badge = if is_dispatch { "\u{25c6} " } else { "" };
    let repo_short = pr.repo.split('/').next_back().unwrap_or(&pr.repo);
    let header = format!(
        "{select_prefix}{dispatch_badge}{repo_short}#{} {}",
        pr.number, pr.title
    );
    let header_truncated = truncate(&header, 60);

    let line1_style = if is_selected || is_cursor {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };

    let ci_color = match pr.ci_status {
        CiStatus::Success => Color::Green,
        CiStatus::Failure => Color::Red,
        CiStatus::Pending => Color::Yellow,
        CiStatus::None => Color::DarkGray,
    };

    let line1 = Line::from(vec![
        Span::styled(stripe, Style::default().fg(color)),
        Span::styled(header_truncated, line1_style),
        Span::styled(
            format!(" {}", pr.ci_status.symbol()),
            Style::default().fg(ci_color),
        ),
    ]);

    // Line 2: author · age · +/-lines
    let meta_parts = [
        format!("@{}", pr.author),
        age,
        format!("+{}/-{}", pr.additions, pr.deletions),
    ];
    let meta = format!("  {} ", meta_parts.join(" \u{b7} "));

    let meta_style = if is_cursor {
        Style::default().fg(Color::Gray)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let line2 = Line::from(Span::styled(meta, meta_style));

    let bg = if is_cursor {
        review_cursor_bg_color(decision_for_color)
    } else {
        Color::Reset
    };

    ListItem::new(vec![line1, line2]).style(Style::default().bg(bg))
}

// ---------------------------------------------------------------------------
// Security board rendering
// ---------------------------------------------------------------------------

use crate::models::{AlertKind, AlertSeverity, SecurityAlert};

fn security_tab_label(app: &App, prefix: &str) -> String {
    let count = app.filtered_security_alerts().len();
    let loading = if app.security_loading() {
        " \u{21bb}"
    } else {
        ""
    };
    if count > 0 {
        format!("{prefix}Security ({count}){loading} ")
    } else {
        format!("{prefix}Security{loading} ")
    }
}

fn security_column_color(severity: AlertSeverity) -> Color {
    match severity {
        AlertSeverity::Critical => Color::Red,
        AlertSeverity::High => YELLOW,
        AlertSeverity::Medium => Color::Rgb(86, 152, 194),
        AlertSeverity::Low => Color::DarkGray,
    }
}

fn security_cursor_bg_color(severity: AlertSeverity) -> Color {
    match severity {
        AlertSeverity::Critical => Color::Rgb(56, 28, 28),
        AlertSeverity::High => Color::Rgb(52, 44, 20),
        AlertSeverity::Medium => Color::Rgb(24, 40, 52),
        AlertSeverity::Low => Color::Rgb(34, 34, 40),
    }
}

fn security_column_bg_color(severity: AlertSeverity) -> Color {
    match severity {
        AlertSeverity::Critical => Color::Rgb(36, 26, 26),
        AlertSeverity::High => Color::Rgb(38, 34, 26),
        AlertSeverity::Medium => Color::Rgb(26, 32, 38),
        AlertSeverity::Low => Color::Rgb(30, 30, 34),
    }
}

pub fn render_security_board(frame: &mut Frame, app: &mut App, area: Rect) {
    let detail_height = if app.security_detail_visible() { 8 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),             // tab bar
            Constraint::Length(1),             // summary row
            Constraint::Min(1),                // board
            Constraint::Length(detail_height), // detail panel
            Constraint::Length(1),             // status bar
        ])
        .split(area);

    render_tab_bar(frame, app, chunks[0]);
    render_security_summary_row(frame, app, chunks[1]);

    let filtered = app.filtered_security_alerts();
    if filtered.is_empty() {
        let msg = if app.security_loading() {
            "Loading..."
        } else if app.filtered_security_alerts().is_empty() && !app.security.alerts.is_empty() {
            "All alerts filtered out."
        } else {
            "No security alerts found. Press r to refresh."
        };
        let p = Paragraph::new(msg)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, chunks[2]);
    } else {
        render_security_columns(frame, app, chunks[2]);
    }

    render_security_detail(frame, app, chunks[3]);

    // Status bar
    if let Some(msg) = app.status_message() {
        let status = Paragraph::new(msg.to_string()).style(Style::default().fg(Color::Yellow));
        frame.render_widget(status, chunks[4]);
    } else if let Some(err) = app.last_security_error() {
        let status = Paragraph::new(format!("Error: {err}")).style(Style::default().fg(Color::Red));
        frame.render_widget(status, chunks[4]);
    } else {
        let has_alert = app.selected_security_alert().is_some();
        let hints = Paragraph::new(Line::from(security_action_hints(app, has_alert)));
        frame.render_widget(hints, chunks[4]);
    }

    // Filter overlay
    if matches!(app.mode(), InputMode::SecurityRepoFilter) {
        render_security_repo_filter_overlay(frame, app, area);
    }
}

fn security_action_hints(app: &App, has_alert: bool) -> Vec<Span<'static>> {
    let key_color = Color::Cyan;
    let label_style = Style::default().fg(MUTED);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let push_hint = |spans: &mut Vec<Span<'static>>, key: &'static str, label: String| {
        spans.push(Span::styled(
            key,
            Style::default().fg(key_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}  "), label_style));
    };
    if has_alert {
        push_hint(&mut spans, "Enter", "detail".into());
        push_hint(&mut spans, "p", "open".into());
    }
    push_hint(&mut spans, "r", "refresh".into());
    push_hint(&mut spans, "f", "filter".into());
    let kind_label = match app.security_kind_filter() {
        None => "all",
        Some(AlertKind::Dependabot) => "deps",
        Some(AlertKind::CodeScanning) => "code",
    };
    push_hint(&mut spans, "t", format!("kind:{kind_label}"));
    push_hint(&mut spans, "Tab", "tasks".into());
    push_hint(&mut spans, "?", "help".into());
    spans
}

fn render_security_summary_row(frame: &mut Frame, app: &App, area: Rect) {
    let sel = app.security_selection();
    let selected_col = sel.map(|s| s.column()).unwrap_or(0);
    let filtered = app.filtered_security_alerts();
    let col_count = AlertSeverity::COLUMN_COUNT;

    let segments = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, col_count as u32); col_count])
        .split(area);

    for i in 0..col_count {
        let severity = AlertSeverity::from_column_index(i).unwrap_or(AlertSeverity::Medium);
        let count = filtered
            .iter()
            .filter(|a| a.severity.column_index() == i)
            .count();
        let is_focused = i == selected_col;
        let prefix = if is_focused { "\u{25b8} " } else { "\u{25e6} " };
        let label = format!("{prefix}{} ({count})", severity.as_str());

        let color = security_column_color(severity);
        let style = if is_focused {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let p = Paragraph::new(label).style(style);
        frame.render_widget(p, segments[i]);
    }
}

fn render_security_columns(frame: &mut Frame, app: &mut App, area: Rect) {
    let sel_col = app.security_selection().map(|s| s.column()).unwrap_or(0);
    let col_count = AlertSeverity::COLUMN_COUNT;

    let col_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, col_count as u32); col_count])
        .split(area);

    for i in 0..col_count {
        let severity = AlertSeverity::from_column_index(i).unwrap_or(AlertSeverity::Medium);
        let is_focused = i == sel_col;
        let alerts: Vec<&SecurityAlert> = app.security_alerts_for_column(i);

        let selected_row = app.security_selection().map(|s| s.row(i)).unwrap_or(0);
        let items: Vec<ListItem> = alerts
            .iter()
            .enumerate()
            .map(|(row, alert)| {
                build_security_alert_item(alert, severity, is_focused && row == selected_row)
            })
            .collect();

        let bg = if is_focused {
            security_column_bg_color(severity)
        } else {
            Color::Reset
        };

        let list = List::new(items).block(Block::default().style(Style::default().bg(bg)));

        let mut list_state = ListState::default();
        if is_focused {
            list_state.select(Some(selected_row));
        }

        frame.render_stateful_widget(list, col_areas[i], &mut list_state);

        if let Some(sel) = app.security_selection_mut() {
            sel.list_states[i] = list_state;
        }
    }
}

fn build_security_alert_item(
    alert: &SecurityAlert,
    severity: AlertSeverity,
    is_cursor: bool,
) -> ListItem<'static> {
    let color = security_column_color(severity);
    let now = Utc::now();
    let age = format_age(alert.created_at, now);

    // Line 1: stripe + repo#number + title
    let stripe = if is_cursor { "\u{258c} " } else { "\u{258e} " };
    let repo_short = alert.repo.split('/').next_back().unwrap_or(&alert.repo);
    let header = format!("{repo_short}#{} {}", alert.number, alert.title);
    let header_truncated = truncate(&header, 55);

    let line1_style = if is_cursor {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };

    let line1 = Line::from(vec![
        Span::styled(stripe, Style::default().fg(color)),
        Span::styled(header_truncated, line1_style),
    ]);

    // Line 2: kind indicator + package + CVSS + age
    let kind_indicator = alert.kind.indicator();
    let pkg = alert.package.as_deref().unwrap_or("-");
    let cvss_str = alert
        .cvss_score
        .map(|s| format!("CVSS:{s:.1}"))
        .unwrap_or_default();
    let meta = format!("  [{kind_indicator}] {pkg} {cvss_str} {age}");

    let meta_style = if is_cursor {
        Style::default().fg(Color::Gray)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let line2 = Line::from(Span::styled(meta, meta_style));

    let bg = if is_cursor {
        security_cursor_bg_color(severity)
    } else {
        Color::Reset
    };

    ListItem::new(vec![line1, line2]).style(Style::default().bg(bg))
}

fn render_security_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(BORDER));

    if !app.security_detail_visible() {
        let paragraph = Paragraph::new("").block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let Some(alert) = app.selected_security_alert() else {
        let paragraph = Paragraph::new("").block(block);
        frame.render_widget(paragraph, area);
        return;
    };

    let color = security_column_color(alert.severity);
    let now = Utc::now();
    let age = format_age(alert.created_at, now);

    // Line 1: title
    let line1 = Line::from(vec![Span::styled(
        format!("{}#{} {}", alert.repo, alert.number, alert.title),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )]);

    // Line 2: kind + severity + CVSS
    let cvss_str = alert
        .cvss_score
        .map(|s| format!(" CVSS:{s:.1}"))
        .unwrap_or_default();
    let line2 = Line::from(Span::styled(
        format!(
            "{} \u{00b7} {}{} \u{00b7} {} \u{00b7} {}",
            alert.kind.as_str(),
            alert.severity.as_str(),
            cvss_str,
            alert.repo,
            age,
        ),
        Style::default().fg(MUTED),
    ));

    // Line 3: package info or location
    let pkg_line = if let Some(pkg) = &alert.package {
        let range = alert.vulnerable_range.as_deref().unwrap_or("");
        let fix = alert
            .fixed_version
            .as_ref()
            .map(|v| format!(" \u{2192} {v}"))
            .unwrap_or_default();
        format!("Package: {pkg} {range}{fix}")
    } else {
        "No package info".to_string()
    };
    let line3 = Line::from(Span::styled(pkg_line, Style::default().fg(MUTED_LIGHT)));

    // Lines 4+: description (truncated)
    let desc_lines: Vec<Line> = alert
        .description
        .lines()
        .take(4)
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(Color::DarkGray),
            ))
        })
        .collect();

    let mut lines = vec![line1, line2, line3];
    lines.extend(desc_lines);

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn render_security_repo_filter_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let repos = app.active_security_repos();
    if repos.is_empty() {
        return;
    }

    let mode_label = match app.security_kind_filter() {
        None => "all kinds",
        Some(AlertKind::Dependabot) => "dependabot only",
        Some(AlertKind::CodeScanning) => "code scanning only",
    };
    let _filter_label = format!("Filter ({mode_label})");

    let height = (repos.len() as u16 + 4).min(area.height.saturating_sub(2));
    let width = 50.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Filter Repos ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(CYAN));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines: Vec<Line> = Vec::new();

    let mode_str = match app.view_mode() {
        ViewMode::SecurityBoard { .. } => match &app.security.repo_filter_mode {
            RepoFilterMode::Include => "include",
            RepoFilterMode::Exclude => "exclude",
        },
        _ => "include",
    };
    lines.push(Line::from(Span::styled(
        format!(" Mode: {mode_str} [Tab to toggle]"),
        Style::default().fg(MUTED_LIGHT),
    )));
    lines.push(Line::from(Span::styled(
        " (a) toggle all",
        Style::default().fg(MUTED),
    )));

    for (i, repo) in repos.iter().enumerate() {
        let is_selected = app.security.repo_filter.contains(repo);
        let marker = if is_selected { "\u{25c9}" } else { "\u{25cb}" };
        let num = i + 1;
        let line = Line::from(vec![
            Span::styled(
                format!(" {num}"),
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {marker} {repo}"),
                if is_selected {
                    Style::default().fg(FG)
                } else {
                    Style::default().fg(MUTED)
                },
            ),
        ]);
        lines.push(line);
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TaskTag;
    use crate::tui::types::TaskDraft;
    use std::time::Duration;

    fn make_test_app() -> App {
        App::new(vec![], Duration::from_secs(300))
    }

    fn dummy_style() -> Style {
        Style::default()
    }

    #[test]
    fn input_description_shows_tag_when_set() {
        let mut app = make_test_app();
        app.input.task_draft = Some(TaskDraft {
            title: "My task".into(),
            tag: Some(TaskTag::Bug),
            ..Default::default()
        });
        app.input.buffer = "some desc".into();
        let lines = input_description_lines(&app, dummy_style(), dummy_style(), dummy_style());
        let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Tag: bug"), "expected tag line, got:\n{text}");
        assert!(text.contains("Title: My task"));
        assert!(text.contains("Description: some desc_"));
    }

    #[test]
    fn input_description_shows_none_when_no_tag() {
        let mut app = make_test_app();
        app.input.task_draft = Some(TaskDraft {
            title: "No tag task".into(),
            tag: None,
            ..Default::default()
        });
        let lines = input_description_lines(&app, dummy_style(), dummy_style(), dummy_style());
        let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Tag: none"), "expected 'Tag: none', got:\n{text}");
    }

    #[test]
    fn input_repo_path_shows_tag_when_set() {
        let mut app = make_test_app();
        app.input.task_draft = Some(TaskDraft {
            title: "Feature task".into(),
            description: "A description".into(),
            tag: Some(TaskTag::Feature),
            ..Default::default()
        });
        app.input.buffer = "/some/path".into();
        let area = Rect::new(0, 0, 80, 24);
        let lines = input_repo_path_lines(&app, area, dummy_style(), dummy_style(), dummy_style());
        let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Tag: feature"), "expected tag line, got:\n{text}");
        assert!(text.contains("Title: Feature task"));
        assert!(text.contains("Description: A description"));
        assert!(text.contains("Repo path: /some/path_"));
    }

    #[test]
    fn input_repo_path_shows_none_when_no_tag() {
        let mut app = make_test_app();
        app.input.task_draft = Some(TaskDraft {
            title: "Plain task".into(),
            description: "desc".into(),
            tag: None,
            ..Default::default()
        });
        app.input.buffer.clear();
        let area = Rect::new(0, 0, 80, 24);
        let lines = input_repo_path_lines(&app, area, dummy_style(), dummy_style(), dummy_style());
        let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Tag: none"), "expected 'Tag: none', got:\n{text}");
    }

    #[test]
    fn format_tokens_below_1000() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_at_and_above_1000() {
        assert_eq!(format_tokens(1000), "1k");
        assert_eq!(format_tokens(1999), "1k");
        assert_eq!(format_tokens(12_345), "12k");
    }

    #[test]
    fn format_usage_compact() {
        use crate::models::TaskId;
        use chrono::Utc;
        let u = TaskUsage {
            task_id: TaskId(1),
            cost_usd: 0.45,
            input_tokens: 12_345,
            output_tokens: 2_000,
            cache_read_tokens: 500,
            cache_write_tokens: 100,
            updated_at: Utc::now(),
        };
        assert_eq!(format_usage(&u), "$0.45 \u{00b7} 12k in / 2k out");
    }
}
