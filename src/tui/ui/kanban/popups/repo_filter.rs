//! Repo filter overlay (with preset management).

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::ui::palette::{CYAN, FG, MUTED, MUTED_LIGHT, YELLOW};
use crate::tui::{App, InputMode, RepoFilterMode};

pub(in crate::tui::ui::kanban) fn render_repo_filter_overlay(
    frame: &mut Frame,
    app: &App,
    area: Rect,
) {
    let is_filter_mode = matches!(
        app.mode(),
        InputMode::RepoFilter
            | InputMode::InputPresetName
            | InputMode::ConfirmDeletePreset
            | InputMode::ConfirmDeleteRepoPath
    );
    if !is_filter_mode {
        return;
    }

    let repo_count = app.board.repo_paths.len();
    let preset_count = app.filter_presets().len();
    let preset_lines = if preset_count > 0 {
        preset_count + 2
    } else {
        0
    };
    let input_line = if matches!(app.mode(), InputMode::InputPresetName) {
        1
    } else {
        0
    };
    // +7: blank(1) + toggle_row(1) + blank(1) + 2_help_lines(2) + borders(2)
    let popup_height = (repo_count as u16 + preset_lines as u16 + input_line as u16 + 7)
        .clamp(8, area.height.saturating_sub(4));
    let popup_width = (area.width * 70 / 100).clamp(30, 60);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    // Repos scroll: cursor 0 = toggle row (not a repo), cursor 1..=N = repo index cursor-1.
    let cursor = app.input.repo_cursor;
    let repo_cursor = cursor.saturating_sub(1);
    let content_height = popup_height.saturating_sub(2) as usize;
    // non_repo = blank(1) + preset_lines + toggle_row(1) + blank(1) + 2_help_lines(2)
    let non_repo_lines = preset_lines + input_line + 5;
    let visible_repos = content_height.saturating_sub(non_repo_lines).max(1);
    let scroll = if repo_count <= visible_repos {
        0
    } else {
        repo_cursor
            .saturating_sub(visible_repos - 1)
            .min(repo_count - visible_repos)
    };

    frame.render_widget(Clear, popup_area);

    let mode_label = app.repo_filter_mode().as_str();
    let block = Block::default()
        .title(format!(" Repo Filter ({mode_label}) "))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(CYAN))
        .title_style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD));

    let key_style = Style::default().fg(CYAN).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(MUTED_LIGHT);
    let note_style = Style::default().fg(MUTED);
    let cursor_style = Style::default().fg(CYAN).add_modifier(Modifier::BOLD);

    let mut lines = vec![Line::from("")];

    // Presets section
    if !app.filter_presets().is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  Presets:",
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
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

    // Toggle row — "Active sessions only"
    let toggle_checked = if app.filter_only_active() { "x" } else { " " };
    let (toggle_indicator, toggle_style) = if cursor == 0 {
        ("  ►", cursor_style)
    } else {
        ("   ", desc_style)
    };
    lines.push(Line::from(vec![
        Span::styled(toggle_indicator, toggle_style),
        Span::styled(
            format!(" [{toggle_checked}] Active sessions only"),
            toggle_style,
        ),
    ]));

    // Repo list (scrollable)
    if scroll > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ↑ {} more", scroll),
            note_style,
        )));
    }
    let broken_style = Style::default().fg(MUTED);
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
        let is_broken = app.broken_repo_paths.contains(path);
        let broken_mark = if is_broken { " [!]" } else { "" };
        if i == repo_cursor && cursor > 0 {
            let style = if is_broken {
                broken_style
            } else {
                cursor_style
            };
            lines.push(Line::from(vec![
                Span::styled("  ►", style),
                Span::styled(format!(" [{checked}] {path}{broken_mark}"), style),
            ]));
        } else {
            let num = i + 1;
            let style = if is_broken { broken_style } else { desc_style };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {num}"),
                    if is_broken { broken_style } else { key_style },
                ),
                Span::styled(format!(". [{checked}] {path}{broken_mark}"), style),
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
            Span::styled(app.input_buffer(), Style::default().fg(FG)),
            Span::styled("_", Style::default().fg(MUTED)),
        ]));
    }

    // Help text
    let all_selected = app.repo_filter().len() == app.board.repo_paths.len();
    let a_label = if all_selected {
        "clear all"
    } else {
        "select all"
    };
    match app.mode() {
        InputMode::InputPresetName => {
            lines.push(Line::from(vec![
                Span::styled("  [Enter]", key_style),
                Span::styled(" save  ", note_style),
                Span::styled("[Esc]", key_style),
                Span::styled(" cancel", note_style),
            ]));
        }
        InputMode::ConfirmDeletePreset => {
            lines.push(Line::from(vec![
                Span::styled("  [A-Z]", key_style),
                Span::styled(" delete preset  ", note_style),
                Span::styled("[Esc]", key_style),
                Span::styled(" cancel", note_style),
            ]));
        }
        InputMode::ConfirmDeleteRepoPath => {
            let path_label = app
                .repo_paths()
                .get(app.input.repo_cursor.saturating_sub(1))
                .map(|p| p.as_str())
                .unwrap_or("?");
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  Delete {path_label}?  "),
                    Style::default().fg(YELLOW),
                ),
                Span::styled("y", key_style),
                Span::styled(": yes  ", note_style),
                Span::styled("n/Esc", key_style),
                Span::styled(": cancel", note_style),
            ]));
        }
        _ => {
            lines.push(Line::from(vec![
                Span::styled("  [j/k]", key_style),
                Span::styled(" navigate  ", note_style),
                Span::styled("[Space]", key_style),
                Span::styled(" toggle  ", note_style),
                Span::styled("[a]", key_style),
                Span::styled(format!(" {a_label}  "), note_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  [Tab]", key_style),
                Span::styled(" incl/excl  ", note_style),
                Span::styled("[s]", key_style),
                Span::styled(" save preset  ", note_style),
                Span::styled("[x]", key_style),
                Span::styled(" del preset  ", note_style),
                Span::styled("[q/Esc]", key_style),
                Span::styled(" close", note_style),
            ]));
        }
    }

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup_area);
}
