//! Error popup overlay.

use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::ui::palette::{FG, MUTED, RED};
use crate::tui::App;

pub(in crate::tui::ui::kanban) fn render_error_popup(frame: &mut Frame, app: &App, area: Rect) {
    let Some(error_msg) = &app.status.error_popup else {
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
        .border_style(Style::default().fg(RED))
        .title_style(Style::default().fg(RED).add_modifier(Modifier::BOLD));

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(error_msg.as_str(), Style::default().fg(FG))),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to dismiss",
            Style::default().fg(MUTED),
        )),
    ];

    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, popup_area);
}
