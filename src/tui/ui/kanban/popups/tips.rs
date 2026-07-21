//! Tips & Tricks overlay.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::App;

pub(in crate::tui::ui::kanban) fn render_tips_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let Some(overlay) = &app.interaction.tips else {
        return;
    };
    let Some(tip) = overlay.current_tip() else {
        return;
    };

    // Center: 70% width, 60% height
    let popup_width = (area.width * 70 / 100).clamp(40, 80);
    let popup_height = (area.height * 60 / 100).clamp(12, 30);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let total = overlay.tips.len();
    let current = overlay.index + 1;
    let title = format!(" Tips & Tricks ({current} / {total}) ");

    let block = Block::default()
        .title(title)
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Reserve bottom 2 rows: 1 blank + 1 footer
    let content_height = inner.height.saturating_sub(2);
    let content_area = Rect::new(inner.x, inner.y, inner.width, content_height);
    let footer_area = Rect::new(inner.x, inner.y + content_height + 1, inner.width, 1);

    // Content: tip title (bold) + blank line + body
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            tip.title.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for body_line in tip.body.lines() {
        lines.push(Line::from(Span::styled(
            body_line.to_string(),
            Style::default().fg(Color::Gray),
        )));
    }

    // NEW badge: bottom-right of content area when tip is unseen
    if overlay.is_new() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(inner.width.saturating_sub(5) as usize)),
            Span::styled(
                "NEW",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    let content = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(content, content_area);

    // Footer: key hints
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("h/←", Style::default().fg(Color::Yellow)),
        Span::raw(" "),
        Span::styled("l/→", Style::default().fg(Color::Yellow)),
        Span::raw(" browse  "),
        Span::styled("[n]", Style::default().fg(Color::Yellow)),
        Span::raw(" new only  "),
        Span::styled("[x]", Style::default().fg(Color::Yellow)),
        Span::raw(" never  "),
        Span::styled("[q]", Style::default().fg(Color::Yellow)),
        Span::raw(" close"),
    ]));
    frame.render_widget(footer, footer_area);
}
