//! Error popup overlay.

use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{BorderType, Paragraph, Wrap},
    Frame,
};

use crate::tui::ui::palette::{FG, MUTED, RED};
use crate::tui::ui::shared::{centered_rect, open_overlay, titled_block};
use crate::tui::App;

pub(in crate::tui::ui::kanban) fn render_error_popup(frame: &mut Frame, app: &App, area: Rect) {
    let Some(error_msg) = &app.status.error_popup else {
        return;
    };

    let popup_width = (area.width * 60 / 100).clamp(30, 60);
    let popup_height = 7_u16;
    let popup_area = centered_rect(area, popup_width, popup_height);

    let block = titled_block(RED, BorderType::Thick, " Error ".to_string());

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(error_msg.as_str(), Style::default().fg(FG))),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to dismiss",
            Style::default().fg(MUTED),
        )),
    ];

    let inner = open_overlay(frame, popup_area, block);

    let paragraph = Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, inner);
}
