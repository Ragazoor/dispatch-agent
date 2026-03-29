use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::models::{Epic, Task, TaskStatus, Staleness, format_age};
use super::{App, ColumnItem, InputMode, ViewMode};

/// Column color per status
fn column_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => Color::Rgb(86, 95, 137),
        TaskStatus::Ready => Color::Rgb(122, 162, 247),
        TaskStatus::Running => Color::Rgb(224, 175, 104),
        TaskStatus::Review => Color::Rgb(187, 154, 247),
        TaskStatus::Done => Color::Rgb(158, 206, 106),
        TaskStatus::Archived => Color::Rgb(86, 95, 137),
    }
}

/// Tinted background for the cursor card in each column.
fn cursor_bg_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => Color::Rgb(34, 38, 66),
        TaskStatus::Ready => Color::Rgb(32, 46, 82),
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
        TaskStatus::Ready => Color::Rgb(27, 32, 48),
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
        TaskStatus::Ready => "⬡",
        TaskStatus::Running => "◉",
        TaskStatus::Review => "◎",
        TaskStatus::Done => "✓",
        TaskStatus::Archived => "◦",
    }
}

/// Map a staleness tier to a terminal color.
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

/// Top-level render function.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let now = Utc::now();

    let has_banner = matches!(app.view_mode(), ViewMode::Epic { .. });

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if has_banner {
            vec![
                Constraint::Length(1),   // summary row
                Constraint::Length(4),   // epic banner
                Constraint::Min(6),      // kanban board
                Constraint::Length(8),   // detail panel
                Constraint::Length(1),   // status bar
            ]
        } else {
            vec![
                Constraint::Length(1),   // summary row
                Constraint::Min(6),      // kanban board
                Constraint::Length(8),   // detail panel
                Constraint::Length(1),   // status bar
            ]
        })
        .split(area);

    if has_banner {
        render_summary(frame, app, vertical[0]);
        render_epic_banner(frame, app, vertical[1]);
        render_columns(frame, app, vertical[2], now);
        render_archive_overlay(frame, app, vertical[2], now);
        render_detail(frame, app, vertical[3], now);
        render_status_bar(frame, app, vertical[4]);
    } else {
        render_summary(frame, app, vertical[0]);
        render_columns(frame, app, vertical[1], now);
        render_archive_overlay(frame, app, vertical[1], now);
        render_detail(frame, app, vertical[2], now);
        render_status_bar(frame, app, vertical[3]);
    }

    render_error_popup(frame, app, area);
    render_help_overlay(frame, app, area);
}

fn render_summary(frame: &mut Frame, app: &App, area: Rect) {
    let segments = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [Constraint::Ratio(1, TaskStatus::COLUMN_COUNT as u32); TaskStatus::COLUMN_COUNT]
        )
        .split(area);

    for (col_idx, &status) in TaskStatus::ALL.iter().enumerate() {
        let count = app.column_items_for_status(status).len();
        let is_focused = app.selected_column() == col_idx;
        let color = column_color(status);

        let (prefix, style) = if is_focused {
            ("\u{25b8} ", Style::default()
                .fg(color)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED))
        } else {
            ("\u{25e6} ", Style::default().fg(Color::Rgb(86, 95, 137)))
        };

        let text = format!("{}{} {}", prefix, status.as_str(), count);
        let paragraph = Paragraph::new(text)
            .style(style)
            .alignment(Alignment::Center);
        frame.render_widget(paragraph, segments[col_idx]);
    }
}

/// Format the title text for a task card (line 1 only — status annotations are on line 2).
fn format_task_title(task: &Task, max_title: usize) -> String {
    truncate(&task.title, max_title)
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
    let is_batch_selected = app.selected_tasks.contains(&task.id);
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

    let line1 = Line::from(vec![
        Span::styled(select_prefix.to_string(), title_style),
        Span::styled(stripe_char, stripe_style),
        Span::styled(format!(" {title_text}"), title_style),
    ]);

    // Line 2: metadata
    let is_conflict = app.merge_conflict_tasks().contains(&task.id);
    let is_crashed = app.crashed_tasks().contains(&task.id);
    let is_stale = app.stale_tasks().contains(&task.id);

    let line2 = if is_conflict {
        Line::from(vec![
            Span::raw("   "),
            Span::styled("\u{26a0} merge conflict", Style::default().fg(Color::Red)),
        ])
    } else if is_crashed {
        Line::from(vec![
            Span::raw("   "),
            Span::styled("\u{26a0} crashed", Style::default().fg(Color::Red)),
        ])
    } else if is_stale {
        let mins = app.agents.last_output_change.get(&task.id)
            .map(|t| t.elapsed().as_secs() / 60)
            .unwrap_or(0);
        Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!("\u{25c9} stale \u{00b7} {}m", mins),
                Style::default().fg(Color::Yellow),
            ),
        ])
    } else if status == TaskStatus::Running {
        Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!("{} running", status_icon(status)),
                Style::default().fg(Color::Rgb(86, 95, 137)),
            ),
        ])
    } else if status == TaskStatus::Review && task.needs_input {
        Line::from(vec![
            Span::raw("   "),
            Span::styled(
                "\u{25c9} needs input",
                Style::default().fg(Color::Yellow),
            ),
        ])
    } else {
        let age = format_age(task.updated_at, now);
        let staleness = Staleness::from_age(task.updated_at, now);
        Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!("{} {}", status_icon(status), age),
                Style::default().fg(staleness_color(staleness)),
            ),
        ])
    };

    // Build two-line ListItem
    let mut item = ListItem::new(vec![line1, line2]);

    // Cursor gets tinted background via ListItem::style() for full-width fill
    if is_cursor {
        item = item.style(
            Style::default()
                .bg(cursor_bg_color(status))
                .fg(Color::Rgb(192, 202, 245))
                .add_modifier(Modifier::BOLD),
        );
    }

    item
}

fn render_columns(frame: &mut Frame, app: &App, area: Rect, now: DateTime<Utc>) {
    let column_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [Constraint::Ratio(1, TaskStatus::COLUMN_COUNT as u32); TaskStatus::COLUMN_COUNT]
        )
        .split(area);

    for (col_idx, &status) in TaskStatus::ALL.iter().enumerate() {
        let col_area = column_areas[col_idx];
        let is_focused = app.selected_column() == col_idx;
        let color = column_color(status);

        let column_items = app.column_items_for_status(status);
        let selected_row = app.selected_row()[col_idx];

        let items: Vec<ListItem> = column_items
            .iter()
            .enumerate()
            .map(|(row_idx, item)| {
                let is_cursor = is_focused && row_idx == selected_row;
                match item {
                    ColumnItem::Task(task) => build_task_list_item(task, status, app, now, is_cursor, color),
                    ColumnItem::Epic(epic) => render_epic_item(epic, is_cursor, app, status),
                }
            })
            .collect();

        if is_focused {
            let block = Block::default()
                .style(Style::default().bg(column_bg_color(status)));
            let inner = block.inner(col_area);
            frame.render_widget(block, col_area);
            let list = List::new(items);
            frame.render_widget(list, inner);
        } else {
            let list = List::new(items);
            frame.render_widget(list, col_area);
        }
    }
}

fn render_epic_item(
    epic: &Epic,
    is_cursor: bool,
    app: &App,
    status: TaskStatus,
) -> ListItem<'static> {
    let subtask_statuses: Vec<TaskStatus> = app.tasks()
        .iter()
        .filter(|t| t.epic_id == Some(epic.id) && t.status != TaskStatus::Archived)
        .map(|t| t.status)
        .collect();

    let done_count = subtask_statuses.iter().filter(|s| **s == TaskStatus::Done).count();
    let running_count = subtask_statuses.iter().filter(|s| {
        matches!(s, TaskStatus::Running | TaskStatus::Review)
    }).count();
    let pending_count = subtask_statuses.len() - done_count - running_count;

    let title_text = truncate(&epic.title, 28);

    // Line 1: stripe + title (thicker stripe for cursor)
    let stripe_char = if is_cursor { "\u{258c}" } else { "\u{258e}" };
    let line1 = Line::from(vec![
        Span::raw("  "),
        Span::styled(stripe_char, Style::default().fg(Color::Rgb(187, 154, 247))),
        Span::styled(
            format!(" {title_text}"),
            Style::default().fg(Color::Rgb(187, 154, 247)).add_modifier(Modifier::BOLD),
        ),
    ]);

    // Line 2: EPIC + subtask counts
    let mut meta = " EPIC".to_string();
    if done_count > 0 {
        meta.push_str(&format!(" +{done_count}"));
    }
    if running_count > 0 {
        meta.push_str(&format!(" ~{running_count}"));
    }
    if pending_count > 0 {
        meta.push_str(&format!(" .{pending_count}"));
    }

    let line2 = Line::from(vec![
        Span::raw("   "),
        Span::styled(meta, Style::default().fg(Color::Rgb(86, 95, 137))),
    ]);

    let mut item = ListItem::new(vec![line1, line2]);

    if is_cursor {
        item = item.style(
            Style::default()
                .bg(cursor_bg_color(status))
                .fg(Color::Rgb(192, 202, 245))
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

    let subtask_statuses: Vec<TaskStatus> = app.tasks()
        .iter()
        .filter(|t| t.epic_id == Some(epic.id) && t.status != TaskStatus::Archived)
        .map(|t| t.status)
        .collect();
    let total = subtask_statuses.len();
    let done = subtask_statuses.iter().filter(|s| **s == TaskStatus::Done).count();

    let block = Block::default()
        .title(format!(" Epic: {} ", epic.title))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Magenta))
        .title_style(Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD));

    let desc = truncate(&epic.description, 60);
    let progress = format!("{done}/{total} done");
    let lines = vec![
        Line::from(vec![
            Span::styled(desc, Style::default().fg(Color::Gray)),
            Span::styled(format!("  {progress}"), Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(Span::styled(
            "Esc to return to board",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn render_archive_overlay(frame: &mut Frame, app: &App, area: Rect, now: DateTime<Utc>) {
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
        .title_style(Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD));

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
    frame.render_widget(list, overlay_area);
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect, _now: DateTime<Utc>) {
    // When in input mode, show the input form instead of detail
    if render_input_form(frame, app, area) {
        return;
    }

    // Top border separator
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::Rgb(41, 46, 66)));

    if !app.detail_visible {
        let paragraph = Paragraph::new("").block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let lines: Vec<Line> = if let Some(task) = app.selected_task() {
        let status_color = column_color(task.status);

        // Line 1: title (bold, colored) + inline metadata (dim)
        let mut line1_spans = vec![
            Span::styled(
                task.title.clone(),
                Style::default().fg(status_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" \u{00b7} #{} \u{00b7} {} \u{00b7} {}", task.id, task.status.as_str(), task.repo_path),
                Style::default().fg(Color::Rgb(86, 95, 137)),
            ),
        ];

        // Add crash/stale suffix
        if app.crashed_tasks().contains(&task.id) {
            line1_spans.push(Span::styled(
                " (crashed)",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        } else if app.stale_tasks().contains(&task.id) {
            let mins = app.agents.last_output_change.get(&task.id)
                .map(|t| t.elapsed().as_secs() / 60)
                .unwrap_or(0);
            line1_spans.push(Span::styled(
                format!(" (stale \u{00b7} {}m)", mins),
                Style::default().fg(Color::Yellow),
            ));
        }

        let l = vec![
            Line::from(line1_spans),
            Line::from(Span::styled(
                task.description.clone(),
                Style::default().fg(Color::Rgb(120, 124, 153)),
            )),
        ];

        l
    } else {
        vec![Line::from(Span::styled(
            "No task selected",
            Style::default().fg(Color::Rgb(86, 95, 137)),
        ))]
    };

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Renders the input form in the detail panel area. Returns true if it rendered.
fn render_input_form(frame: &mut Frame, app: &App, area: Rect) -> bool {
    let completed = Style::default().fg(Color::White);
    let active = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let hint = Style::default().fg(Color::DarkGray);

    let lines: Vec<Line> = match &app.input.mode {
        InputMode::InputTitle => {
            vec![
                Line::from(Span::styled(
                    format!("  Title: {}_ ", app.input.buffer),
                    active,
                )),
                Line::from(""),
                Line::from(Span::styled("  Enter to confirm, Esc to cancel", hint)),
            ]
        }
        InputMode::InputDescription => {
            let title = app.input.task_draft.as_ref().map(|d| d.title.as_str()).unwrap_or("");
            vec![
                Line::from(Span::styled(format!("  Title: {title}"), completed)),
                Line::from(Span::styled(
                    format!("  Description: {}_ ", app.input.buffer),
                    active,
                )),
                Line::from(""),
                Line::from(Span::styled("  Enter to confirm, Esc to cancel", hint)),
            ]
        }
        InputMode::InputRepoPath => {
            let title = app.input.task_draft.as_ref().map(|d| d.title.as_str()).unwrap_or("");
            let description = app.input.task_draft.as_ref().map(|d| d.description.as_str()).unwrap_or("");
            let mut lines = vec![
                Line::from(Span::styled(format!("  Title: {title}"), completed)),
                Line::from(Span::styled(
                    format!("  Description: {description}"),
                    completed,
                )),
                Line::from(Span::styled(
                    format!("  Repo path: {}_ ", app.input.buffer),
                    active,
                )),
            ];
            // Show saved repo paths if available and user hasn't started typing
            if app.input.buffer.is_empty() {
                for (i, path) in app.repo_paths.iter().enumerate() {
                    lines.push(Line::from(Span::styled(
                        format!("    [{}] {path}", i + 1),
                        hint,
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Type a path or press 1-9 to select, Enter to confirm, Esc to cancel",
                hint,
            )));
            lines
        }
        InputMode::QuickDispatch => {
            let mut lines = vec![
                Line::from(Span::styled("  Quick Dispatch — select repo:", active)),
                Line::from(""),
            ];
            for (i, path) in app.repo_paths.iter().enumerate() {
                lines.push(Line::from(Span::styled(
                    format!("    [{}] {path}", i + 1),
                    hint,
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Press 1-9 to select, Esc to cancel",
                hint,
            )));
            lines
        }
        InputMode::ConfirmRetry(id) => {
            let label = if app.crashed_tasks().contains(id) {
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
                Line::from(Span::styled("  [r] Resume (--continue in existing worktree)", hint)),
                Line::from(Span::styled("  [f] Fresh start (clean worktree + new dispatch)", hint)),
                Line::from(Span::styled("  [Esc] Cancel", hint)),
            ]
        }
        InputMode::InputEpicTitle => {
            vec![
                Line::from(Span::styled(
                    format!("  Title: {}_ ", app.input.buffer),
                    active,
                )),
                Line::from(""),
                Line::from(Span::styled("  Enter to confirm, Esc to cancel", hint)),
            ]
        }
        InputMode::InputEpicDescription => {
            let title = app.input.epic_draft.as_ref().map(|d| d.title.as_str()).unwrap_or("");
            vec![
                Line::from(Span::styled(format!("  Title: {title}"), completed)),
                Line::from(Span::styled(
                    format!("  Description: {}_ ", app.input.buffer),
                    active,
                )),
                Line::from(""),
                Line::from(Span::styled("  Enter to confirm, Esc to cancel", hint)),
            ]
        }
        InputMode::InputEpicRepoPath => {
            let title = app.input.epic_draft.as_ref().map(|d| d.title.as_str()).unwrap_or("");
            let description = app.input.epic_draft.as_ref().map(|d| d.description.as_str()).unwrap_or("");
            let mut lines = vec![
                Line::from(Span::styled(format!("  Title: {title}"), completed)),
                Line::from(Span::styled(
                    format!("  Description: {description}"),
                    completed,
                )),
                Line::from(Span::styled(
                    format!("  Repo path: {}_ ", app.input.buffer),
                    active,
                )),
            ];
            if app.input.buffer.is_empty() {
                for (i, path) in app.repo_paths.iter().enumerate() {
                    lines.push(Line::from(Span::styled(
                        format!("    [{}] {path}", i + 1),
                        hint,
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Type a path or press 1-9 to select, Enter to confirm, Esc to cancel",
                hint,
            )));
            lines
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

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
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
    let popup_height = (area.height * 80 / 100).clamp(23, 29);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let header = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc = Style::default().fg(Color::Gray);
    let note = Style::default().fg(Color::DarkGray);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Navigation", header)),
        Line::from(vec![
            Span::styled("  h/\u{2190}", key), Span::styled(" previous column   ", desc),
            Span::styled("j/\u{2193}", key), Span::styled(" next task", desc),
        ]),
        Line::from(vec![
            Span::styled("  l/\u{2192}", key), Span::styled(" next column       ", desc),
            Span::styled("k/\u{2191}", key), Span::styled(" previous task", desc),
        ]),
        Line::from(vec![
            Span::styled("  Enter", key), Span::styled(" detail panel / enter epic", desc),
        ]),
        Line::from(vec![
            Span::styled("  Esc", key), Span::styled(" exit epic / clear selection", desc),
        ]),
        Line::from(""),
        Line::from(Span::styled("  Actions", header)),
        Line::from(vec![
            Span::styled("  n", key), Span::styled(" new task   ", desc),
            Span::styled("E", key), Span::styled(" new epic   ", desc),
            Span::styled("e", key), Span::styled(" edit", desc),
        ]),
        Line::from(vec![
            Span::styled("  d", key), Span::styled(" dispatch*  ", desc),
            Span::styled("m", key), Span::styled(" move fwd   ", desc),
            Span::styled("M", key), Span::styled(" move back", desc),
        ]),
        Line::from(vec![
            Span::styled("  x", key), Span::styled(" archive    ", desc),
            Span::styled("D", key), Span::styled(" quick dsp  ", desc),
            Span::styled("g", key), Span::styled(" go to tmux", desc),
        ]),
        Line::from(vec![
            Span::styled("  H", key), Span::styled(" history    ", desc),
            Span::styled("V", key), Span::styled(" epic done  ", desc),
            Span::styled("Space", key), Span::styled(" select", desc),
        ]),
        Line::from(vec![
            Span::styled("  f", key), Span::styled(" finish     ", desc),
            Span::styled("(Review: merge + clean up worktree)", note),
        ]),
        Line::from(""),
        Line::from(Span::styled("  * d is context-dependent:", note)),
        Line::from(Span::styled("    Backlog \u{2192} brainstorm   Ready \u{2192} dispatch", note)),
        Line::from(Span::styled("    Running \u{2192} resume (if window gone)", note)),
        Line::from(""),
        Line::from(Span::styled("  General", header)),
        Line::from(vec![
            Span::styled("  ?", key), Span::styled(" this help  ", desc),
            Span::styled("q", key), Span::styled(" quit", desc),
        ]),
        Line::from(""),
        Line::from(Span::styled("  Press ? or Esc to close", note)),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup_area);
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(msg) = &app.status_message {
        let bar = Paragraph::new(msg.as_str())
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(bar, area);
        return;
    }

    // Archive mode status bar
    if app.show_archived() {
        let key_color = Color::Rgb(86, 95, 137);
        let label_style = Style::default().fg(Color::Rgb(86, 95, 137));
        let spans = vec![
            Span::styled("x", Style::default().fg(key_color).add_modifier(Modifier::BOLD)),
            Span::styled(" delete  ", label_style),
            Span::styled("e", Style::default().fg(key_color).add_modifier(Modifier::BOLD)),
            Span::styled(" edit  ", label_style),
            Span::styled("H", Style::default().fg(key_color).add_modifier(Modifier::BOLD)),
            Span::styled(" close  ", label_style),
            Span::styled("q", Style::default().fg(key_color).add_modifier(Modifier::BOLD)),
            Span::styled(" quit  ", label_style),
        ];
        let bar = Paragraph::new(Line::from(spans));
        frame.render_widget(bar, area);
        return;
    }

    match &app.input.mode {
        InputMode::Normal => {
            let key_color = column_color(TaskStatus::ALL[app.selected_column()]);
            let spans = if !app.selected_tasks.is_empty() {
                batch_action_hints(app.selected_tasks.len(), key_color)
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
            let bar = Paragraph::new("Creating task: enter description")
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::InputRepoPath => {
            let bar = Paragraph::new("Creating task: enter repo path")
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmDelete => {
            let text = app.status_message.as_deref().unwrap_or("Delete? (y/n)");
            let bar = Paragraph::new(text)
                .style(Style::default().fg(Color::Red));
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
            let bar = Paragraph::new("Archive task? (y/n)")
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmFinish(_) => {
            let bar = Paragraph::new("Finish: merge to main and clean up? (y/n)")
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmDone(_) => {
            let text = app.status_message.as_deref().unwrap_or("Move to Done? (y/n)");
            let bar = Paragraph::new(text)
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(bar, area);
        }
        InputMode::InputEpicTitle => {
            let bar = Paragraph::new("Creating epic: enter title")
                .style(Style::default().fg(Color::Magenta));
            frame.render_widget(bar, area);
        }
        InputMode::InputEpicDescription => {
            let bar = Paragraph::new("Creating epic: enter description")
                .style(Style::default().fg(Color::Magenta));
            frame.render_widget(bar, area);
        }
        InputMode::InputEpicRepoPath => {
            let bar = Paragraph::new("Creating epic: enter repo path")
                .style(Style::default().fg(Color::Magenta));
            frame.render_widget(bar, area);
        }
        InputMode::ConfirmDeleteEpic => {
            let text = app.status_message.as_deref().unwrap_or("Delete epic and subtasks? (y/n)");
            let bar = Paragraph::new(text)
                .style(Style::default().fg(Color::Red));
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
    }
}

/// Build context-sensitive keybinding hint spans for the status bar.
/// Returns styled spans showing available actions for the selected task.
pub(in crate::tui) fn action_hints(task: Option<&Task>, key_color: Color) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(Color::Rgb(86, 95, 137));

    let mut spans: Vec<Span<'static>> = Vec::new();

    let mut push_hint = |key: &'static str, label: &'static str| {
        spans.push(Span::styled(key, Style::default().fg(key_color).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {label}  "), label_style));
    };

    if let Some(task) = task {
        match task.status {
            TaskStatus::Backlog => {
                push_hint("d", "brainstorm");
                push_hint("e", "edit");
                push_hint("m", "move");
                push_hint("x", "archive");
            }
            TaskStatus::Ready => {
                push_hint("d", "dispatch");
                push_hint("e", "edit");
                push_hint("m", "move");
                push_hint("M", "back");
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
                    push_hint("f", "finish");
                }
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
            TaskStatus::Done => {
                push_hint("e", "edit");
                push_hint("M", "back");
                push_hint("x", "archive");
            }
            TaskStatus::Archived => {}
        }
    }

    push_hint("n", "new");
    push_hint("E", "epic");
    push_hint("D", "quick");
    push_hint("H", "history");
    push_hint("q", "quit");

    spans
}

/// Build status bar hints when tasks are batch-selected.
fn batch_action_hints(count: usize, key_color: Color) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(Color::Rgb(86, 95, 137));
    let count_style = Style::default().fg(Color::Rgb(224, 175, 104)).add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(format!("{count} selected  "), count_style));

    let mut push_hint = |key: &'static str, label: &'static str| {
        spans.push(Span::styled(key, Style::default().fg(key_color).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {label}  "), label_style));
    };

    push_hint("m", "move");
    push_hint("M", "back");
    push_hint("x", "archive");
    push_hint("Space", "toggle");
    push_hint("Esc", "clear");
    spans
}
