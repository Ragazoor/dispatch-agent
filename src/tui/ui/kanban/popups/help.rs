//! Help overlay.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::{App, InputMode};

pub(in crate::tui::ui::kanban) fn render_help_overlay(frame: &mut Frame, app: &App, area: Rect) {
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

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Navigation", header)),
        Line::from(vec![
            Span::styled("  [h/\u{2190}]", key),
            Span::styled(" prev column     ", desc),
            Span::styled("[j/\u{2193}]", key),
            Span::styled(" next task", desc),
        ]),
        Line::from(vec![
            Span::styled("  [l/\u{2192}]", key),
            Span::styled(" next column     ", desc),
            Span::styled("[k/\u{2191}]", key),
            Span::styled(" prev task", desc),
        ]),
        Line::from(vec![
            Span::styled("  [Enter]", key),
            Span::styled(" task detail      ", desc),
            Span::styled("[e]", key),
            Span::styled(" edit / enter epic", desc),
        ]),
        Line::from(vec![
            Span::styled("  [q]", key),
            Span::styled(" exit epic   ", desc),
            Span::styled("[Esc]", key),
            Span::styled(" clear sel   ", desc),
            Span::styled("[gg/[ / G/]]", key),
            Span::styled(" jump top/bottom", desc),
        ]),
        Line::from(""),
        Line::from(Span::styled("  Actions", header)),
        Line::from(vec![
            Span::styled("  [n]", key),
            Span::styled(" new task   ", desc),
            Span::styled("[E]", key),
            Span::styled(" new epic   ", desc),
            Span::styled("[N]", key),
            Span::styled(" notifications", desc),
        ]),
        Line::from(vec![
            Span::styled("  [m]", key),
            Span::styled(" reparent epic / move task to epic (tree picker)", desc),
        ]),
        Line::from(vec![
            Span::styled("  [d]", key),
            Span::styled(" dispatch*  ", desc),
            Span::styled("[H/L]", key),
            Span::styled(" move task/epic backward/forward", desc),
        ]),
        Line::from(vec![
            Span::styled("  [x]", key),
            Span::styled(" done/arch  ", desc),
            Span::styled("[D]", key),
            Span::styled(" quick dsp  ", desc),
            Span::styled("[Space]", key),
            Span::styled(" session/board (gg: top)", desc),
        ]),
        Line::from(vec![
            Span::styled("  [l/\u{2192}]", key),
            Span::styled(" Archive   ", desc),
            Span::styled("[a]", key),
            Span::styled(" select all   ", desc),
            Span::styled("[/]", key),
            Span::styled(" search titles", desc),
        ]),
        Line::from(vec![
            Span::styled("  [v]", key),
            Span::styled(" select  ", desc),
            Span::styled("[f]", key),
            Span::styled(" filter repos  ", desc),
            Span::styled("[A]", key),
            Span::styled(" filter active  ", desc),
            Span::styled("[W]", key),
            Span::styled(" wrap up", desc),
        ]),
        Line::from(vec![
            Span::styled("  [T]", key),
            Span::styled(" detach tmux panel  ", desc),
            Span::styled("(any task with a tmux window, supports batch)", note),
        ]),
        Line::from(vec![
            Span::styled("  [s]", key),
            Span::styled(" toggle split  ", desc),
            Span::styled("[S]", key),
            Span::styled(" swap task into pane", desc),
        ]),
        Line::from(vec![
            Span::styled("  [p]", key),
            Span::styled(" open PR in browser  ", desc),
            Span::styled("[:]", key),
            Span::styled(" main session (jump / pick dir)", desc),
        ]),
        Line::from(vec![
            Span::styled("  [J/K]", key),
            Span::styled(" reorder item up/down  ", desc),
            Span::styled("[C]", key),
            Span::styled(" feed config  ", desc),
            Span::styled("[P]", key),
            Span::styled(" todo (a add \u{b7} space done \u{b7} J/K order \u{b7} e edit \u{b7} c clear-done \u{b7} d del \u{b7} t add)", desc),
        ]),
        Line::from(""),
        Line::from(Span::styled("  * [d] is context-dependent:", note)),
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
            Span::styled("  [?]", key),
            Span::styled(" this help  ", desc),
            Span::styled("[N]", key),
            Span::styled(" notify on/off  ", desc),
            Span::styled("[q]", key),
            Span::styled(" quit (or exit epic)", desc),
        ]),
        Line::from(vec![
            Span::styled("  Prefix+Space", key),
            Span::styled(" back to board  ", desc),
            Span::styled("Prefix+e", key),
            Span::styled(" toggle tree  ", desc),
            Span::styled("(tmux)", note),
        ]),
        Line::from(""),
        Line::from(Span::styled("  [?] or [Esc] to close", note)),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup_area);
}
