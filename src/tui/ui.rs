use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
};

use crate::models::TaskStatus;
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

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),      // kanban board
            Constraint::Length(8),   // detail panel
            Constraint::Length(1),   // status bar
        ])
        .split(area);

    render_columns(frame, app, vertical[0]);
    render_detail(frame, app, vertical[1]);
    render_status_bar(frame, app, vertical[2]);
}

fn render_columns(frame: &mut Frame, app: &App, area: Rect) {
    let column_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
        ])
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

        let title = format!(" {} ", status.as_str().to_uppercase());
        let block = Block::default()
            .title(title)
            .title_style(title_style)
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(border_style);

        let tasks = app.tasks_by_status(status);
        let selected_row = app.selected_row[col_idx];

        let items: Vec<ListItem> = tasks
            .iter()
            .enumerate()
            .map(|(row_idx, task)| {
                let is_selected = is_focused && row_idx == selected_row;

                // For Running tasks, show last line of tmux output as a hint
                let label = if status == TaskStatus::Running {
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
                    truncate(&task.title, 38)
                };

                let style = if is_selected {
                    Style::default()
                        .bg(color)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                ListItem::new(Line::from(Span::styled(label, style)))
            })
            .collect();

        let list = List::new(items).block(block);

        frame.render_widget(list, col_area);
    }
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let content = if app.detail_visible {
        if let Some(text) = &app.detail_text {
            text.as_str()
        } else if let Some(task) = app.selected_task() {
            // Build a temporary display (we can't own a String here, use the stored text)
            // detail_text should have been set by ToggleDetail; fall back gracefully.
            task.title.as_str()
        } else {
            "No task selected"
        }
    } else {
        ""
    };

    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(msg) = &app.status_message {
        msg.as_str().to_string()
    } else {
        match &app.mode {
            InputMode::Normal => {
                "q:quit  h/l:col  j/k:row  n:new  m/M:move  d:dispatch  Enter:detail  x:delete"
                    .to_string()
            }
            InputMode::InputTitle => {
                format!("Title> {}", app.input_buffer)
            }
            InputMode::InputDescription { .. } => {
                format!("Description> {}", app.input_buffer)
            }
            InputMode::InputRepoPath { .. } => {
                format!("Repo path> {}", app.input_buffer)
            }
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
