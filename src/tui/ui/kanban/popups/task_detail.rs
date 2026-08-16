//! Task detail overlay (peek/zoom).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::ui::palette::{BORDER, FG, MUTED, MUTED_LIGHT};
use crate::tui::{App, ViewMode};

use crate::tui::ui::shared::{open_overlay, rounded_block};

use super::super::wrapped_line_count;

pub(in crate::tui::ui::kanban) fn render_task_detail_overlay(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
) {
    let (task_id, scroll, zoomed) = match &app.board.view_mode {
        ViewMode::TaskDetail {
            task_id,
            scroll,
            zoomed,
            ..
        } => (*task_id, *scroll, *zoomed),
        _ => return,
    };

    let Some(task) = app.board.tasks.iter().find(|t| t.id == task_id).cloned() else {
        return;
    };

    // Compute overlay area
    let overlay_height = if zoomed {
        area.height.saturating_sub(1) // full height minus status bar
    } else {
        area.height / 2
    };
    let overlay_y = area.bottom().saturating_sub(overlay_height + 1); // above status bar
    let overlay_area = Rect {
        x: area.x,
        y: overlay_y,
        width: area.width,
        height: overlay_height,
    };

    // ── Header lines (metadata) ──────────────────────────────────────────────
    let label_style = Style::default().fg(MUTED);
    let value_style = Style::default().fg(FG);
    let mut header_lines: Vec<Line> = Vec::with_capacity(4);
    let mut field = |label: &'static str, value: String| {
        header_lines.push(Line::from(vec![
            Span::styled(label, label_style),
            Span::styled(value, value_style),
        ]));
    };

    field("Repo:  ", task.repo_path.clone());

    if let Some(epic_id) = task.epic_id {
        let epic_title = app.epic_title(epic_id).unwrap_or("").to_string();
        field("Epic:  ", format!("#{} — {}", epic_id, epic_title));
    }

    if let Some(u) = &task.url {
        let field_label = match u.url_type {
            crate::models::UrlType::Pr => "PR:    ",
            crate::models::UrlType::Issue => "Issue: ",
            crate::models::UrlType::SecurityAlert => "Alert: ",
            crate::models::UrlType::Other => "Link:  ",
        };
        field(field_label, u.url.clone());
    }

    if let Some(plan_path) = &task.plan_path {
        field("Plan:  ", plan_path.clone());
    }

    let header_height = header_lines.len() as u16 + 1; // +1 for separator line

    // ── Compute body area and scroll clamping ────────────────────────────────
    let body_height = overlay_area.height.saturating_sub(2 + header_height + 1); // borders(2) + header + separator(1)
    let body_width = overlay_area.width.saturating_sub(2) as usize;

    let desc_wrapped = wrapped_line_count(&task.description, body_width);
    let new_max_scroll = desc_wrapped.saturating_sub(body_height as usize) as u16;

    if let ViewMode::TaskDetail {
        ref mut max_scroll, ..
    } = app.board.view_mode
    {
        if *max_scroll != new_max_scroll {
            *max_scroll = new_max_scroll;
        }
    }

    // ── Block with hints ─────────────────────────────────────────────────────
    let hint_style = Style::default().fg(MUTED);
    let block = rounded_block(BORDER)
        .title(format!(" Task #{task_id} "))
        .title_bottom(Line::from(Span::styled(
            " j/k scroll · z zoom · q/Esc/Enter close ",
            hint_style,
        )));

    let inner = open_overlay(frame, overlay_area, block);

    // ── Render header inside block ────────────────────────────────────────────
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(header_height), Constraint::Min(0)])
        .split(inner);

    let header_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(BORDER));
    frame.render_widget(Paragraph::new(header_lines).block(header_block), layout[0]);

    // ── Render scrollable description ─────────────────────────────────────────
    let desc_lines: Vec<Line> = task
        .description
        .lines()
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(MUTED_LIGHT),
            ))
        })
        .collect();

    frame.render_widget(
        Paragraph::new(desc_lines)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        layout[1],
    );
}
