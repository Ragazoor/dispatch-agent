//! Personal TODO overlay — render function.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{BorderType, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use super::palette::{BLUE, CYAN, MUTED, PURPLE, YELLOW};
use super::shared::{centered_rect, open_overlay, titled_block};
use crate::models::TodoLink;
use crate::tui::{App, InputMode, ViewMode};

/// Checkbox glyph for an open (not-done) todo.
const OPEN_GLYPH: &str = "▢";
/// Checkbox glyph for a completed (done) todo.
const DONE_GLYPH: &str = "▣";

pub fn render_todos(frame: &mut Frame, app: &App, area: Rect) {
    let ViewMode::Todos {
        ref todos,
        selected,
        ..
    } = app.board.view_mode
    else {
        return;
    };

    // ── Centered overlay (70% × 70%) ──────────────────────────────────────────
    let overlay_width = (area.width * 70 / 100).clamp(40, 100);
    let overlay_height = (area.height * 70 / 100).clamp(12, 35);
    let overlay_area = centered_rect(area, overlay_width, overlay_height);

    let open_count = todos.iter().filter(|t| !t.done).count();
    let title = format!(" TODO ({open_count} open) ");

    let outer_block = titled_block(CYAN, BorderType::Rounded, title);

    let inner_area = open_overlay(frame, overlay_area, outer_block);

    let adding = matches!(
        app.input.mode,
        InputMode::TodoTitle | InputMode::TodoQuickAdd
    );

    // Reserve rows: footer(1) + input row(1) when in add/edit mode.
    let bottom_reserve: u16 = if adding { 2 } else { 1 };
    let list_height = inner_area.height.saturating_sub(bottom_reserve);
    let list_area = Rect {
        height: list_height,
        ..inner_area
    };

    let items: Vec<ListItem> = todos
        .iter()
        .map(|todo| {
            let is_child = todo.parent_id.is_some();
            let indent = if is_child { "  " } else { "" };
            if todo.done {
                // Done items: dimmed + strikethrough — no badge
                ListItem::new(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(
                        DONE_GLYPH,
                        Style::default().fg(MUTED).add_modifier(Modifier::DIM),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        todo.title.clone(),
                        Style::default()
                            .fg(MUTED)
                            .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
                    ),
                ]))
            } else {
                let mut spans = vec![
                    Span::raw(indent),
                    Span::styled(OPEN_GLYPH, Style::default().fg(CYAN)),
                    Span::raw(" "),
                    Span::raw(todo.title.clone()),
                ];
                // Append colored ID badge for linked items
                if let Some(link) = todo.linked {
                    spans.push(Span::raw("  "));
                    let (badge, color) = match link {
                        TodoLink::Task(id) => (format!("[task #{}]", id.0), BLUE),
                        TodoLink::Epic(id) => (format!("[epic #{}]", id.0), PURPLE),
                    };
                    spans.push(Span::styled(badge, Style::default().fg(color)));
                }
                ListItem::new(Line::from(spans))
            }
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(if todos.is_empty() {
        None
    } else {
        Some(selected)
    });

    let list = List::new(items).highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    frame.render_stateful_widget(list, list_area, &mut list_state);

    // Input row — visible when add mode is active.
    if adding {
        let input_area = Rect {
            y: inner_area.y + list_height,
            height: 1,
            ..inner_area
        };
        let line = crate::tui::ui::caret_field_line(
            input_area.width,
            " > ",
            "",
            &app.input.buffer,
            app.input.caret,
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        );
        frame.render_widget(Paragraph::new(line), input_area);
    }

    // Footer hints
    let footer_area = Rect {
        y: inner_area.y + inner_area.height.saturating_sub(1),
        height: 1,
        ..inner_area
    };
    let hint_text = if adding {
        " [Enter] save  [Esc] cancel"
    } else {
        " a add · space done · J/K order · Tab nest · S-Tab unnest · e edit · L link · U unlink · Enter/g jump · c clear-done · d del · q back"
    };
    let hints = Paragraph::new(hint_text).style(Style::default().fg(MUTED));
    frame.render_widget(hints, footer_area);
}
