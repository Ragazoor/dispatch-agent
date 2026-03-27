use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::models::{TaskStatus, Staleness, format_age};
use super::{App, InputMode};

/// Column color per status
fn column_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => Color::DarkGray,
        TaskStatus::Ready => Color::Blue,
        TaskStatus::Running => Color::Yellow,
        TaskStatus::Review => Color::Magenta,
        TaskStatus::Done => Color::Green,
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

/// Format age for the detail panel — slightly more verbose than card labels.
fn format_detail_age(updated_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let age = now.signed_duration_since(updated_at);
    let total_hours = age.num_hours().max(0);

    if total_hours < 1 {
        "less than 1 hour".to_string()
    } else if total_hours == 1 {
        "1 hour".to_string()
    } else if total_hours < 24 {
        format!("{total_hours} hours")
    } else {
        let days = total_hours / 24;
        if days == 1 {
            "1 day".to_string()
        } else {
            format!("{days} days")
        }
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

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),      // kanban board
            Constraint::Length(8),   // detail panel
            Constraint::Length(1),   // status bar
        ])
        .split(area);

    render_columns(frame, app, vertical[0], now);
    render_detail(frame, app, vertical[1], now);
    render_status_bar(frame, app, vertical[2]);

    render_error_popup(frame, app, area);
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
        let is_focused = app.selected_column == col_idx;
        let color = column_color(status);

        let (border_type, border_style, title_style) = if is_focused {
            (
                BorderType::Double,
                Style::default().fg(color),
                Style::default().bg(color).fg(Color::Black).add_modifier(Modifier::BOLD),
            )
        } else {
            (
                BorderType::Plain,
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::DarkGray),
            )
        };

        let tasks = app.tasks_by_status(status);
        let title = format!(" {} ({}) ", status.as_str().to_uppercase(), tasks.len());
        let block = Block::default()
            .title(title)
            .title_style(title_style)
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(border_style);
        let selected_row = app.selected_row[col_idx];

        let items: Vec<ListItem> = tasks
            .iter()
            .enumerate()
            .map(|(row_idx, task)| {
                let is_selected = is_focused && row_idx == selected_row;
                let show_age = status != TaskStatus::Running;

                // Build the age suffix for non-Running tasks
                let age_suffix = if show_age {
                    format!(" {}", format_age(task.updated_at, now))
                } else {
                    String::new()
                };

                let max_title = 38_usize.saturating_sub(age_suffix.len());

                // Build the title portion
                let title_text = if status == TaskStatus::Running {
                    if let Some(output) = app.tmux_outputs.get(&task.id) {
                        let last_line = output.lines().last().unwrap_or("").trim();
                        if !last_line.is_empty() {
                            format!(
                                "{} [{}]",
                                truncate(&task.title, 20),
                                truncate(last_line, 15)
                            )
                        } else {
                            truncate(&task.title, 38)
                        }
                    } else {
                        truncate(&task.title, 38)
                    }
                } else {
                    truncate(&task.title, max_title)
                };

                if is_selected {
                    // Selected: uniform inverted style (readability over staleness color)
                    let selected_style = Style::default()
                        .bg(color)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD);
                    let full_label = format!("{title_text}{age_suffix}");
                    ListItem::new(Line::from(Span::styled(full_label, selected_style)))
                } else if show_age && !age_suffix.is_empty() {
                    // Unselected with age: two spans (title default, age colored)
                    let staleness = Staleness::from_age(task.updated_at, now);
                    let age_style = Style::default().fg(staleness_color(staleness));
                    ListItem::new(Line::from(vec![
                        Span::styled(title_text, Style::default()),
                        Span::styled(age_suffix, age_style),
                    ]))
                } else {
                    // Running or no age
                    ListItem::new(Line::from(Span::styled(title_text, Style::default())))
                }
            })
            .collect();

        let list = List::new(items).block(block);

        frame.render_widget(list, col_area);
    }
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect, now: DateTime<Utc>) {
    // When in input mode, show the input form instead of detail
    if render_input_form(frame, app, area) {
        return;
    }

    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    if !app.detail_visible {
        let paragraph = Paragraph::new("").block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let lines: Vec<Line> = if let Some(task) = app.selected_task() {
        let mut l = vec![
            Line::from(format!(
                "ID: {}  Status: {}  Repo: {}",
                task.id,
                task.status.as_str(),
                task.repo_path
            )),
            Line::from(format!("Title: {}", task.title)),
            Line::from(format!("Description: {}", task.description)),
            Line::from(format!(
                "Plan: {}",
                task.plan.as_deref().unwrap_or("-")
            )),
            Line::from(format!(
                "Worktree: {}  Tmux: {}",
                task.worktree.as_deref().unwrap_or("-"),
                task.tmux_window.as_deref().unwrap_or("-")
            )),
            Line::from(format!(
                "Updated: {} ago",
                format_detail_age(task.updated_at, now)
            )),
        ];
        if let Some(output) = app.tmux_outputs.get(&task.id) {
            l.push(Line::from(""));
            for line in output.lines() {
                l.push(Line::from(line.to_string()));
            }
        }
        l
    } else {
        vec![Line::from("No task selected")]
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

    let lines: Vec<Line> = match &app.mode {
        InputMode::InputTitle => {
            vec![
                Line::from(Span::styled(
                    format!("  Title: {}_ ", app.input_buffer),
                    active,
                )),
                Line::from(""),
                Line::from(Span::styled("  Enter to confirm, Esc to cancel", hint)),
            ]
        }
        InputMode::InputDescription => {
            let title = app.task_draft.as_ref().map(|d| d.title.as_str()).unwrap_or("");
            vec![
                Line::from(Span::styled(format!("  Title: {title}"), completed)),
                Line::from(Span::styled(
                    format!("  Description: {}_ ", app.input_buffer),
                    active,
                )),
                Line::from(""),
                Line::from(Span::styled("  Enter to confirm, Esc to cancel", hint)),
            ]
        }
        InputMode::InputRepoPath => {
            let title = app.task_draft.as_ref().map(|d| d.title.as_str()).unwrap_or("");
            let description = app.task_draft.as_ref().map(|d| d.description.as_str()).unwrap_or("");
            let mut lines = vec![
                Line::from(Span::styled(format!("  Title: {title}"), completed)),
                Line::from(Span::styled(
                    format!("  Description: {description}"),
                    completed,
                )),
                Line::from(Span::styled(
                    format!("  Repo path: {}_ ", app.input_buffer),
                    active,
                )),
            ];
            // Show saved repo paths if available and user hasn't started typing
            if app.input_buffer.is_empty() {
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

    let block = Block::default()
        .title(" New Task ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let paragraph = Paragraph::new(lines).block(block);
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

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(msg) = &app.status_message {
        msg.as_str().to_string()
    } else {
        match &app.mode {
            InputMode::Normal => {
                "q:quit  h/l:col  j/k:row  n:new  e:edit  m/M:move  d:dispatch  Enter:detail  x:delete"
                    .to_string()
            }
            InputMode::InputTitle => "Creating task: enter title".to_string(),
            InputMode::InputDescription => "Creating task: enter description".to_string(),
            InputMode::InputRepoPath => "Creating task: enter repo path".to_string(),
            InputMode::ConfirmDelete => "Delete? (y/n)".to_string(),
        }
    };

    let style = match app.mode {
        InputMode::Normal => Style::default().fg(Color::DarkGray),
        _ => Style::default().fg(Color::Yellow),
    };

    let bar = Paragraph::new(text).style(style);
    frame.render_widget(bar, area);
}
