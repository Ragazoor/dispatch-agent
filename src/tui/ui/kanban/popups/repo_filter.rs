//! Repo filter overlay (with preset management).

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{BorderType, Paragraph},
    Frame,
};

use crate::tui::ui::palette::{CYAN, FG, MUTED, YELLOW};
use crate::tui::ui::shared::{
    centered_rect, open_overlay, scroll_offset, titled_block, visible_rows, HintStyles,
};
use crate::tui::{App, InputMode, RepoFilterMode};

/// Top and bottom border rows of the popup block.
const BORDER_ROWS: usize = 2;

/// Shortest popup worth drawing, when the board has the room for it.
const MIN_POPUP_HEIGHT: u16 = 8;

/// Everything the overlay's geometry needs, derived once.
struct RepoFilterLayout {
    popup_area: Rect,
    /// Repo rows that fit in the list window (at least one).
    visible_repos: usize,
    /// Index of the first repo shown.
    scroll: usize,
}

/// Derive the overlay's popup rect and its scrolling repo window.
///
/// `non_repo_rows` is the number of content rows the caller has *already built*
/// — the header block above the repo list plus the footer block below it. It is
/// counted from those line vectors rather than hand-tallied, so the popup height
/// (which adds it) and the visible-repo window (which subtracts it) cannot drift
/// apart, and neither can drift from what the overlay actually draws. The two
/// hand-counted literals this replaced (`+7` and `+5`, budgeted from opposite
/// directions with nothing checking they agreed) were exactly that hazard.
///
/// `repo_cursor` is the cursor's index into the repo list — the toggle row is
/// not a repo, so callers pass `cursor - 1`.
fn repo_filter_layout(
    area: Rect,
    repo_count: usize,
    non_repo_rows: usize,
    repo_cursor: usize,
) -> RepoFilterLayout {
    let wanted_height = repo_count + non_repo_rows + BORDER_ROWS;
    // The board keeps two rows of margin above and below the popup. On a board
    // too short to honour `MIN_POPUP_HEIGHT`, the ceiling wins — a bare
    // `.clamp(MIN_POPUP_HEIGHT, ceiling)` would panic on `min > max`, and
    // render code must never panic.
    let ceiling = area.height.saturating_sub(4);
    let popup_height = u16::try_from(wanted_height)
        .unwrap_or(u16::MAX)
        .clamp(MIN_POPUP_HEIGHT.min(ceiling), ceiling);
    let popup_width = (area.width * 70 / 100).clamp(30, 60);

    let content_height = popup_height.saturating_sub(BORDER_ROWS as u16) as usize;
    let visible_repos = visible_rows(content_height, non_repo_rows);

    RepoFilterLayout {
        popup_area: centered_rect(area, popup_width, popup_height),
        visible_repos,
        scroll: scroll_offset(repo_cursor, repo_count, visible_repos),
    }
}

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
    // Repos scroll: cursor 0 = toggle row (not a repo), cursor 1..=N = repo index cursor-1.
    let cursor = app.input.repo_cursor;
    let styles = HintStyles::new(CYAN);

    // Build everything except the repo list first, so the layout can be sized
    // from the rows that will actually be drawn rather than from a tally that
    // has to be kept in step by hand.
    let mut header = vec![Line::from("")];
    append_preset_section(&mut header, app, &styles);
    header.push(toggle_row_line(app, cursor, &styles));

    let mut footer = vec![Line::from("")];
    if matches!(app.mode(), InputMode::InputPresetName) {
        footer.push(preset_name_input_line(app, &styles));
    }
    append_help_lines(&mut footer, app, &styles);

    let layout = repo_filter_layout(
        area,
        repo_count,
        header.len() + footer.len(),
        cursor.saturating_sub(1),
    );

    let mode_label = app.repo_filter_mode().as_str();
    let block = titled_block(
        CYAN,
        BorderType::Double,
        format!(" Repo Filter ({mode_label}) "),
    );

    let mut lines = header;
    append_repo_list(&mut lines, app, &layout, cursor, &styles);
    lines.append(&mut footer);

    let inner = open_overlay(frame, layout.popup_area, block);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The "Presets:" header, one lettered row per saved preset, and a trailing
/// blank. Appends nothing when no presets exist.
fn append_preset_section<'a>(lines: &mut Vec<Line<'a>>, app: &'a App, styles: &HintStyles) {
    if app.filter_presets().is_empty() {
        return;
    }
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
            Span::styled(format!("  {letter}"), styles.accent),
            Span::styled(format!(". {name}{mode_tag}"), styles.desc),
        ]));
    }
    lines.push(Line::from(""));
}

/// The "Active sessions only" checkbox row, which the cursor treats as index 0.
fn toggle_row_line(app: &App, cursor: usize, styles: &HintStyles) -> Line<'static> {
    let toggle_checked = if app.filter_only_active() { "x" } else { " " };
    let (indicator, style) = if cursor == 0 {
        ("  ►", styles.accent)
    } else {
        ("   ", styles.desc)
    };
    Line::from(vec![
        Span::styled(indicator, style),
        Span::styled(format!(" [{toggle_checked}] Active sessions only"), style),
    ])
}

/// The scrolling repo checkbox list, bracketed by "↑ N more" / "↓ N more"
/// markers when the window doesn't cover the whole list.
fn append_repo_list<'a>(
    lines: &mut Vec<Line<'a>>,
    app: &'a App,
    layout: &RepoFilterLayout,
    cursor: usize,
    styles: &HintStyles,
) {
    let repo_cursor = cursor.saturating_sub(1);
    let broken_style = Style::default().fg(MUTED);

    if layout.scroll > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ↑ {} more", layout.scroll),
            styles.note,
        )));
    }
    for (i, path) in app
        .repo_paths()
        .iter()
        .enumerate()
        .skip(layout.scroll)
        .take(layout.visible_repos)
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
                styles.accent
            };
            lines.push(Line::from(vec![
                Span::styled("  ►", style),
                Span::styled(format!(" [{checked}] {path}{broken_mark}"), style),
            ]));
        } else {
            let num = i + 1;
            let style = if is_broken { broken_style } else { styles.desc };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {num}"),
                    if is_broken {
                        broken_style
                    } else {
                        styles.accent
                    },
                ),
                Span::styled(format!(". [{checked}] {path}{broken_mark}"), style),
            ]));
        }
    }
    // Counted from the same accessor the loop above iterates, so the "N more"
    // tail can never disagree with the rows actually drawn.
    let remaining = app
        .repo_paths()
        .len()
        .saturating_sub(layout.scroll + layout.visible_repos);
    if remaining > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ↓ {} more", remaining),
            styles.note,
        )));
    }
}

/// The preset-name entry row, shown only in `InputMode::InputPresetName`.
fn preset_name_input_line<'a>(app: &'a App, styles: &HintStyles) -> Line<'a> {
    Line::from(vec![
        Span::styled("  Name: ", styles.accent),
        Span::styled(app.input_buffer(), Style::default().fg(FG)),
        Span::styled("_", Style::default().fg(MUTED)),
    ])
}

/// Footer help, one branch per input mode the overlay can be in.
fn append_help_lines<'a>(lines: &mut Vec<Line<'a>>, app: &'a App, styles: &HintStyles) {
    match app.mode() {
        InputMode::InputPresetName => lines.push(save_preset_help_line(styles)),
        InputMode::ConfirmDeletePreset => lines.push(delete_preset_help_line(styles)),
        InputMode::ConfirmDeleteRepoPath => lines.push(delete_repo_path_help_line(app, styles)),
        _ => append_browse_help_lines(lines, app, styles),
    }
}

fn save_preset_help_line(styles: &HintStyles) -> Line<'static> {
    Line::from(vec![
        Span::styled("  [Enter]", styles.accent),
        Span::styled(" save  ", styles.note),
        Span::styled("[Esc]", styles.accent),
        Span::styled(" cancel", styles.note),
    ])
}

fn delete_preset_help_line(styles: &HintStyles) -> Line<'static> {
    Line::from(vec![
        Span::styled("  [A-Z]", styles.accent),
        Span::styled(" delete preset  ", styles.note),
        Span::styled("[Esc]", styles.accent),
        Span::styled(" cancel", styles.note),
    ])
}

fn delete_repo_path_help_line<'a>(app: &'a App, styles: &HintStyles) -> Line<'a> {
    let path_label = app
        .repo_paths()
        .get(app.input.repo_cursor.saturating_sub(1))
        .map(|p| p.as_str())
        .unwrap_or("?");
    Line::from(vec![
        Span::styled(
            format!("  Delete {path_label}?  "),
            Style::default().fg(YELLOW),
        ),
        Span::styled("y", styles.accent),
        Span::styled(": yes  ", styles.note),
        Span::styled("n/Esc", styles.accent),
        Span::styled(": cancel", styles.note),
    ])
}

/// The two-row default footer. The other three modes render one row; the
/// layout budget is counted from whichever this produces, not assumed.
fn append_browse_help_lines<'a>(lines: &mut Vec<Line<'a>>, app: &App, styles: &HintStyles) {
    let all_selected = app.repo_filter().len() == app.board.repo_paths.len();
    let a_label = if all_selected {
        "clear all"
    } else {
        "select all"
    };
    lines.extend([
        Line::from(vec![
            Span::styled("  [j/k]", styles.accent),
            Span::styled(" navigate  ", styles.note),
            Span::styled("[Space]", styles.accent),
            Span::styled(" toggle  ", styles.note),
            Span::styled("[a]", styles.accent),
            Span::styled(format!(" {a_label}  "), styles.note),
        ]),
        Line::from(vec![
            Span::styled("  [Tab]", styles.accent),
            Span::styled(" incl/excl  ", styles.note),
            Span::styled("[s]", styles.accent),
            Span::styled(" save preset  ", styles.note),
            Span::styled("[x]", styles.accent),
            Span::styled(" del preset  ", styles.note),
            Span::styled("[q/Esc]", styles.accent),
            Span::styled(" close", styles.note),
        ]),
    ]);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A board tall enough that the popup height never hits its clamp.
    const ROOMY: Rect = Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 60,
    };

    /// With the budget counted from the rows the caller actually built, an
    /// unclamped popup shows every repo with nothing left over — no scroll
    /// markers, no blank filler row. This is the invariant the old `+7`/`+5`
    /// pair of hand-counted literals had no way to guarantee.
    ///
    /// On its own this is close to an identity; the check with teeth is
    /// `repo_filter_renders_its_whole_footer_in_every_mode`, which drives the real
    /// render path and so pins `non_repo_rows` against the lines drawn.
    #[test]
    fn layout_reserves_exactly_the_rows_the_popup_budgets() {
        for repo_count in 1..=12 {
            for non_repo_rows in [5, 6, 8, 10] {
                let l = repo_filter_layout(ROOMY, repo_count, non_repo_rows, 0);
                assert_eq!(
                    l.visible_repos, repo_count,
                    "repo_count={repo_count} non_repo_rows={non_repo_rows}"
                );
                assert_eq!(l.scroll, 0);
            }
        }
    }

    #[test]
    fn popup_height_grows_with_the_non_repo_rows() {
        let base = repo_filter_layout(ROOMY, 4, 5, 0).popup_area.height;
        assert_eq!(
            repo_filter_layout(ROOMY, 4, 8, 0).popup_area.height,
            base + 3
        );
        assert_eq!(
            repo_filter_layout(ROOMY, 4, 6, 0).popup_area.height,
            base + 1
        );
    }

    #[test]
    fn popup_is_centered_and_clamped_to_the_board() {
        // 40 repos cannot fit in a 20-row board: height clamps to height-4.
        let area = Rect::new(0, 0, 100, 20);
        let l = repo_filter_layout(area, 40, 5, 0);
        assert_eq!(l.popup_area.height, 16);
        assert_eq!(l.popup_area.width, 60); // 70% of 100 clamped to 60
        assert_eq!(l.popup_area, centered_rect(area, 60, 16));
    }

    #[test]
    fn popup_height_has_a_floor_of_eight_rows() {
        // No repos at all still leaves a usable box.
        assert_eq!(repo_filter_layout(ROOMY, 0, 5, 0).popup_area.height, 8);
    }

    #[test]
    fn window_scrolls_to_keep_the_repo_cursor_visible() {
        let area = Rect::new(0, 0, 100, 20);
        let l = repo_filter_layout(area, 40, 5, 39);
        // Content height 14, minus the 5 non-repo rows → 9 visible repos.
        assert_eq!(l.visible_repos, 9);
        assert_eq!(l.scroll, 40 - 9);
    }

    #[test]
    fn visible_repos_never_falls_below_one() {
        // A tiny board where the chrome alone exceeds the content height.
        let area = Rect::new(0, 0, 100, 10);
        let l = repo_filter_layout(area, 30, 14, 0);
        assert_eq!(l.visible_repos, 1);
    }

    /// A board too short for `MIN_POPUP_HEIGHT` must yield the tallest popup
    /// that fits, not panic. Render code has no room to unwind.
    #[test]
    fn short_boards_shrink_the_popup_instead_of_panicking() {
        for height in 0..=12u16 {
            let l = repo_filter_layout(Rect::new(0, 0, 100, height), 5, 5, 0);
            assert!(l.popup_area.height <= height.saturating_sub(4));
            assert!(l.visible_repos >= 1);
        }
    }
}
