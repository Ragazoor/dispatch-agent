use super::palette::{FG, GREEN, MUTED, MUTED_LIGHT, RED, YELLOW};

use crate::models::{FeedRole, Staleness};
use crate::tui::{App, RepoFilterMode, ViewMode};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, ListItem, Paragraph},
    Frame,
};
use std::time::{Duration, Instant};

/// Returns (text, color) for the 1-line refresh status row shown on Review and Security boards.
///
/// Thresholds are relative to `interval` so both boards stay consistent regardless of their
/// different poll rates.
pub fn refresh_status(
    last_fetch: Option<Instant>,
    loading: bool,
    interval: Duration,
) -> (String, Color) {
    if loading {
        return ("Refreshing...  [r] refresh".to_string(), MUTED);
    }
    let Some(last) = last_fetch else {
        return ("Never fetched  [r] refresh".to_string(), MUTED);
    };
    let elapsed = last.elapsed();
    let elapsed_str = if elapsed.as_secs() < 60 {
        format!("{}s ago", elapsed.as_secs())
    } else {
        format!(
            "{}m {}s ago",
            elapsed.as_secs() / 60,
            elapsed.as_secs() % 60
        )
    };
    let text = format!("Updated {elapsed_str}  [r] refresh");
    let color = if elapsed >= interval * 4 {
        RED
    } else if elapsed >= interval * 2 {
        YELLOW
    } else {
        FG
    };
    (text, color)
}

/// Map a staleness tier to a terminal color.
/// Uses indexed terminal colors (not palette constants) so these adapt to the
/// user's terminal theme rather than being locked to Tokyo Night RGB values.
pub(in crate::tui::ui) fn staleness_color(staleness: Staleness) -> Color {
    match staleness {
        Staleness::Fresh => Color::Green,
        Staleness::Aging => Color::Yellow,
        Staleness::Stale => Color::Red,
    }
}

/// A `Block` with all four sides and rounded corners, border-styled with `color`.
/// Callers chain `.title(...)` / `.title_style(...)` / `.style(...)` as needed.
pub(in crate::tui::ui) fn rounded_block(color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
}

/// A fully-bordered block whose border and title share `color`, with the title
/// bolded — the frame every overlay in this codebase draws.
///
/// [`rounded_block`] is the untitled `BorderType::Rounded` case; pass
/// `BorderType::Rounded` here when a rounded overlay also wants a title.
pub(in crate::tui::ui) fn titled_block(
    color: Color,
    border_type: BorderType,
    title: String,
) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(color))
        .title_style(Style::default().fg(color).add_modifier(Modifier::BOLD))
}

/// The style set for keyed hint rows — the `[key] description` tables the help
/// and repo-filter overlays are built from.
///
/// `accent` covers everything that should stand out against the body text:
/// key glyphs, section headers, and the selection cursor. They were three
/// separately-declared locals with byte-identical values before, in two
/// different files; one field means a recolour cannot make them disagree.
pub(in crate::tui::ui) struct HintStyles {
    pub accent: Style,
    pub desc: Style,
    pub note: Style,
}

impl HintStyles {
    pub fn new(accent_color: Color) -> Self {
        Self {
            accent: Style::default()
                .fg(accent_color)
                .add_modifier(Modifier::BOLD),
            desc: Style::default().fg(MUTED_LIGHT),
            note: Style::default().fg(MUTED),
        }
    }
}

/// Rows left for a scrolling list once `reserved` rows of surrounding chrome
/// are taken out of `total`, floored at one so a list is never invisible.
///
/// Paired with [`scroll_offset`]: every caller computes this, then feeds it
/// straight in. Keeping them adjacent means a change to the floor lands on both.
pub(in crate::tui::ui) fn visible_rows(total: usize, reserved: usize) -> usize {
    total.saturating_sub(reserved).max(1)
}

/// Left edge of a `width`-wide box centred horizontally inside `area`.
///
/// Saturating so an over-wide box pins to `area.x` rather than wrapping.
pub(in crate::tui::ui) fn centered_x(area: Rect, width: u16) -> u16 {
    area.x + area.width.saturating_sub(width) / 2
}

/// A `width` × `height` `Rect` centred inside `area`, never larger than it.
///
/// The single home for the centred-overlay offset arithmetic that the error,
/// help, repo-filter and todos overlays all need. Overlays that pin one axis
/// (the tree pickers pin their top edge) use [`centered_x`] directly.
///
/// Every caller sizes itself as a clamped percentage of the board, and those
/// clamps have floors (the help overlay's is 25 rows) that exceed a short
/// terminal — so the requested box is capped to `area` here rather than at each
/// call site. [`open_overlay`] repeats the guard against the real buffer.
pub(in crate::tui::ui) fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        centered_x(area, width),
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Open an overlay: clear `popup_area`, draw `block` into it, and return the
/// block's inner area for the caller's content.
///
/// Keeps the clear-then-block order (and the inner-area derivation) in one
/// place so every overlay frames its content identically.
///
/// `popup_area` is intersected with the frame first. `Frame::render_widget`
/// does no clipping of its own and `Clear` writes every cell it is given, so an
/// overlay whose height floor exceeds the terminal's would otherwise panic the
/// render thread with "index outside of buffer". Because every overlay's
/// content rect derives from the returned inner area, one guard here covers the
/// whole class — including the top-pinned tree pickers, which `centered_rect`
/// never sees.
pub(in crate::tui::ui) fn open_overlay<'a>(
    frame: &mut Frame,
    popup_area: Rect,
    block: Block<'a>,
) -> Rect {
    let popup_area = popup_area.intersection(frame.area());
    frame.render_widget(Clear, popup_area);
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    inner
}

/// First visible index for a list of `count` items showing `visible` rows with
/// the cursor at `cursor`.
///
/// The window scrolls only as far as needed to keep the cursor on the last
/// visible row, and never past the end of the list. Shared by the repo-path
/// picker (`input_form`) and the repo-filter overlay, which previously carried
/// verbatim copies of this formula.
pub(in crate::tui::ui) fn scroll_offset(cursor: usize, count: usize, visible: usize) -> usize {
    if visible == 0 || count <= visible {
        0
    } else {
        cursor.saturating_sub(visible - 1).min(count - visible)
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

/// Separator joining ancestor-breadcrumb segments, shared by the flat-view
/// epic header (`fair_truncate_segments`) and the epic-view border title.
pub const BREADCRUMB_SEPARATOR: &str = " › ";

/// Join `segments` with `sep`, truncating so the joined result fits within
/// `budget` display characters while keeping a visible prefix of every segment.
///
/// Water-filling: segments shorter than the fair share are kept whole and their
/// unused width is redistributed to longer segments. Falls back to a
/// whole-string truncate when the budget is too small to give every segment at
/// least a one-char prefix plus ellipsis.
pub fn fair_truncate_segments(segments: &[&str], budget: usize, sep: &str) -> String {
    if segments.is_empty() {
        return String::new();
    }
    let n = segments.len();
    let sep_cost = (n - 1) * sep.chars().count();
    let lens: Vec<usize> = segments.iter().map(|s| s.chars().count()).collect();
    let name_budget = budget.saturating_sub(sep_cost);

    // Everything already fits (separators included).
    if sep_cost + lens.iter().sum::<usize>() <= budget {
        return segments.join(sep);
    }
    // Too narrow to give each segment ≥1 char + ellipsis: truncate the whole string.
    if name_budget < 2 * n {
        return truncate(&segments.join(sep), budget);
    }

    // Water-fill: lock any segment at or under the current fair share, return its
    // slack to the pool, and repeat until only oversized segments remain.
    let mut alloc = vec![0usize; n];
    let mut settled = vec![false; n];
    let mut pool = name_budget;
    let mut remaining = n;
    loop {
        let fair = pool / remaining;
        let mut locked_any = false;
        for i in 0..n {
            if !settled[i] && lens[i] <= fair {
                alloc[i] = lens[i];
                settled[i] = true;
                pool -= lens[i];
                remaining -= 1;
                locked_any = true;
            }
        }
        if !locked_any || remaining == 0 {
            break;
        }
    }
    // Split the remaining pool across still-oversized segments; leftmost get the
    // remainder chars first.
    if let Some(base) = pool.checked_div(remaining) {
        let mut extra = pool % remaining;
        for i in 0..n {
            if !settled[i] {
                alloc[i] = base + usize::from(extra > 0);
                extra = extra.saturating_sub(1);
            }
        }
    }

    segments
        .iter()
        .enumerate()
        .map(|(i, s)| truncate(s, alloc[i]))
        .collect::<Vec<_>>()
        .join(sep)
}

/// Compact indicator for an epic's feed routing role, shown in the epic header.
/// `None` for ordinary epics (`FeedRole::None`); `Some("role:<role>  ")` for
/// managed feed epics so the routing parent and its role sub-epics are
/// identifiable at a glance.
pub(in crate::tui::ui) fn feed_role_label(role: FeedRole) -> Option<String> {
    match role {
        FeedRole::None => None,
        other => Some(format!("role:{}  ", other.as_str())),
    }
}

pub(in crate::tui::ui) fn render_top_indicators(frame: &mut Frame, app: &App, area: Rect) {
    let mut parts: Vec<Span> = Vec::new();
    // Auto dispatch indicator — only in epic view
    if let ViewMode::Epic { epic_id, .. } = app.view_mode() {
        if let Some(epic) = app.epics().iter().find(|e| e.id == *epic_id) {
            let (label, style) = if epic.auto_dispatch {
                ("auto dispatch [U]  ", Style::default().fg(GREEN))
            } else {
                ("manual dispatch [U]  ", Style::default().fg(MUTED))
            };
            parts.push(Span::styled(label, style));

            // Feed routing role — for managed feed epics (parent + role sub-epics)
            if let Some(role_label) = feed_role_label(epic.feed_role) {
                parts.push(Span::styled(role_label, Style::default().fg(MUTED)));
            }

            // Append-only marker — read-only, and shown ONLY when set
            // (epics.allium: AppendOnlyEpicIndicator). No bracketed key hint:
            // a bracket here means "press this to toggle", and the OFF
            // direction of this flag hands the next feed cycle licence to
            // remove every accumulated task the emission does not name
            // (feeds.allium: AppendOnlyFeed), so it is wiring, not a keypress.
            if epic.feed_append_only {
                parts.push(Span::styled("append-only  ", Style::default().fg(GREEN)));
            }

            // Group-by-repo indicator — shown for all epics
            let (label, style) = if epic.group_by_repo {
                ("group:on [R]  ", Style::default().fg(GREEN))
            } else {
                ("group:off [R]  ", Style::default().fg(MUTED))
            };
            parts.push(Span::styled(label, style));
        }
    }
    if !app.repo_filter().is_empty() {
        let active = app.repo_filter().len();
        let total = app.board.repo_paths.len();
        let label = match app.repo_filter_mode() {
            RepoFilterMode::Include => format!("[{active}/{total} repos]  "),
            RepoFilterMode::Exclude => format!("[excl {active}/{total} repos]  "),
        };
        parts.push(Span::styled(label, Style::default().fg(MUTED)));
    }
    if app.notifications_enabled() {
        parts.push(Span::styled("\u{1F514} [N]", Style::default().fg(YELLOW)));
    } else {
        parts.push(Span::styled("\u{1F515} [N]", Style::default().fg(MUTED)));
    }

    // Budget indicator is prepended so it sits left of everything else, and is
    // given only the width the existing badges leave over — it is never allowed
    // to push them off-screen (dispatch.allium: TokenBudgetIndicator degradation).
    let used_width: usize = parts.iter().map(|s| s.content.chars().count()).sum();
    // `used_width` sums codepoints, but the bell/no-bell badge
    // ("\u{1F514} [N]" / "\u{1F515} [N]") is one codepoint narrower than its
    // display width — the emoji renders double-width under ratatui's
    // unicode-width measurement, which `Paragraph`/`Alignment::Right` actually
    // use to lay out and truncate the line. Reserve one extra column here so
    // that undercount can never make the composed line one column too wide
    // and get its right edge (the pre-existing badges) clipped. Do not remove
    // this as a stray off-by-one — see docs/specs/dispatch.allium's
    // `@guarantee DegradesWhenRowTooNarrow`.
    let budget_width_budget = (area.width as usize).saturating_sub(used_width + 1);
    let budget = super::budget::budget_spans(
        app.budget.as_ref(),
        chrono::Utc::now().timestamp(),
        crate::tui::BUDGET_STALE_AFTER,
        budget_width_budget,
    );
    parts.splice(0..0, budget);

    let line = Line::from(parts);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Right), area);
}

/// Non-selectable section header injected between substatus groups.
/// `first` — when true, omits the leading blank line so the top of the column
/// doesn't have an awkward gap before the very first group.
pub(in crate::tui::ui) fn render_substatus_header(label: &str, first: bool) -> ListItem<'static> {
    let header = Line::from(Span::styled(
        format!("  \u{2500}\u{2500} {label} "),
        Style::default().fg(FG).add_modifier(Modifier::BOLD),
    ));
    if first {
        ListItem::new(vec![header])
    } else {
        ListItem::new(vec![Line::raw(""), header])
    }
}

/// Push a keybinding hint as styled spans.
///
/// When the key is a single char matching the label's first letter (e.g. `d` / `dispatch`),
/// renders the compact `[d]ispatch` form. Otherwise renders `[key] label`.
pub(in crate::tui::ui) fn push_hint_spans(
    spans: &mut Vec<Span<'static>>,
    key: &str,
    label: &str,
    key_color: Color,
    label_style: Style,
) {
    let mut key_chars = key.chars();
    let key_char = key_chars.next();
    let key_is_single = key_char.is_some() && key_chars.next().is_none();
    let can_embed = key_is_single
        && label
            .chars()
            .next()
            .zip(key_char)
            .is_some_and(|(l, k)| l.eq_ignore_ascii_case(&k));

    spans.push(Span::styled(
        format!("[{key}]"),
        Style::default().fg(key_color).add_modifier(Modifier::BOLD),
    ));
    let label_text = if can_embed {
        format!("{}  ", &label[1..])
    } else {
        format!(" {label}  ")
    };
    spans.push(Span::styled(label_text, label_style));
}

/// Render a single-line text field with the caret drawn as a reversed block
/// cell at its position, replacing the old trailing-`_` fake caret.
///
/// `prefix` is the (already-styled-as-`base`) label, e.g. `"  Title: "`.
/// `caret` is a **char** index into `buffer` (`0..=chars().count()`).
/// `value_width` is the number of terminal columns available for the value
/// after the prefix; when the buffer is longer, the value is horizontally
/// scrolled so the caret stays visible (the caret is anchored near the right
/// edge of the window). A trailing space cell carries the caret when it sits at
/// the end of the buffer.
pub(in crate::tui) fn caret_line(
    prefix: String,
    buffer: &str,
    caret: usize,
    value_width: usize,
    base: Style,
) -> Line<'static> {
    let caret_style = base.add_modifier(Modifier::REVERSED);
    let chars: Vec<char> = buffer.chars().collect();
    let len = chars.len();
    let caret = caret.min(len);
    let width = value_width.max(1);

    // Lay out one cell per char, plus a trailing blank cell for the caret when
    // it sits at the end. Scroll so the caret is always inside [start, end).
    // The window right-anchors the caret when scrolled: as the user arrows left
    // past the left edge, `start` decreases one cell at a time (the caret sits
    // at the rightmost visible cell, then the text shifts). Simple and always
    // keeps the caret visible; no leading margin before scrolling kicks in.
    let total = if caret == len { len + 1 } else { len };
    let start = if total > width {
        caret.saturating_sub(width - 1)
    } else {
        0
    };
    let end = (start + width).min(total);

    let mut before = String::new();
    let mut after = String::new();
    let mut caret_cell = ' ';
    // Index loop is intentional: `i` ranges up to `len` (one past the last char)
    // to place the caret cell when the caret is at the end, and is compared to
    // `caret`/`len` — not just used to index — so an iterator rewrite would drop
    // the at-end cell.
    #[allow(clippy::needless_range_loop)]
    for i in start..end {
        if i < caret {
            before.push(chars[i]);
        } else if i == caret {
            caret_cell = if i < len { chars[i] } else { ' ' };
        } else {
            after.push(chars[i]);
        }
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, base));
    }
    if !before.is_empty() {
        spans.push(Span::styled(before, base));
    }
    spans.push(Span::styled(caret_cell.to_string(), caret_style));
    if !after.is_empty() {
        spans.push(Span::styled(after, base));
    }
    Line::from(spans)
}

/// Render a labelled single-line field with an optional trailing hint.
///
/// Budgets the value width from the total `area_width` minus the prefix and
/// suffix, renders the caret line via [`caret_line`], and appends the suffix
/// span. This is the shared skeleton for every active text-input row (the input
/// popup rows, the status-bar todo row, the todos overlay row).
pub(in crate::tui) fn caret_field_line(
    area_width: u16,
    prefix: &str,
    suffix: &str,
    buffer: &str,
    caret: usize,
    base: Style,
) -> Line<'static> {
    let value_width = (area_width as usize)
        .saturating_sub(prefix.chars().count() + suffix.chars().count())
        .max(1);
    let mut line = caret_line(prefix.to_string(), buffer, caret, value_width, base);
    if !suffix.is_empty() {
        line.spans.push(Span::styled(suffix.to_string(), base));
    }
    line
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Concatenate a line's span contents back into a plain string.
    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The reversed caret cell's content (there is exactly one reversed span).
    fn caret_cell(line: &Line) -> String {
        line.spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.to_string())
            .expect("a caret cell")
    }

    #[test]
    fn caret_line_mid_string_highlights_char_at_caret() {
        let line = caret_line("  Title: ".to_string(), "abc", 1, 80, Style::default());
        // full text = prefix + buffer (no scrolling at width 80)
        assert_eq!(line_text(&line), "  Title: abc");
        // caret at index 1 -> highlights 'b'
        assert_eq!(caret_cell(&line), "b");
    }

    #[test]
    fn caret_line_at_end_appends_block_cell() {
        let line = caret_line(String::new(), "ab", 2, 80, Style::default());
        // trailing blank caret cell
        assert_eq!(line_text(&line), "ab ");
        assert_eq!(caret_cell(&line), " ");
    }

    #[test]
    fn caret_line_scrolls_to_keep_caret_visible_at_end() {
        // 20-char buffer, width 10, caret at end: window shows the tail and the
        // caret cell is present.
        let buf = "0123456789abcdefghij";
        let caret = buf.chars().count(); // 20, at end
        let line = caret_line(String::new(), buf, caret, 10, Style::default());
        let text = line_text(&line);
        // exactly `width` cells rendered (9 tail chars + 1 caret space)
        assert_eq!(text.chars().count(), 10);
        // the beginning is scrolled off
        assert!(!text.contains('0'));
        // the caret (blank) cell is the last cell
        assert!(text.ends_with(' '));
        assert_eq!(caret_cell(&line), " ");
    }

    #[test]
    fn caret_line_shows_start_when_caret_at_zero() {
        let buf = "0123456789abcdefghij";
        let line = caret_line(String::new(), buf, 0, 10, Style::default());
        let text = line_text(&line);
        assert!(text.starts_with('0'));
        assert_eq!(caret_cell(&line), "0");
    }

    // ---- centered_rect / centered_x --------------------------------------

    #[test]
    fn centered_rect_centers_within_area() {
        let area = Rect::new(0, 0, 100, 40);
        let r = centered_rect(area, 60, 10);
        assert_eq!(r, Rect::new(20, 15, 60, 10));
        // Equal slack on both sides.
        assert_eq!(r.x - area.x, area.right() - r.right());
        assert_eq!(r.y - area.y, area.bottom() - r.bottom());
    }

    #[test]
    fn centered_rect_honours_a_non_zero_area_origin() {
        let area = Rect::new(5, 3, 20, 10);
        assert_eq!(centered_rect(area, 10, 4), Rect::new(10, 6, 10, 4));
    }

    #[test]
    fn centered_rect_pins_to_origin_when_box_is_larger_than_area() {
        // Saturating: an over-sized box must not wrap around to the far edge.
        let area = Rect::new(4, 2, 10, 6);
        let r = centered_rect(area, 30, 20);
        assert_eq!(r.x, 4);
        assert_eq!(r.y, 2);
    }

    #[test]
    fn centered_rect_never_exceeds_its_area() {
        // Overlays size themselves as a clamped percentage of the board, and
        // those clamps have floors taller than a short terminal. The box has to
        // shrink rather than hang off the edge — an over-hanging rect panics
        // `Clear` with "index outside of buffer".
        let area = Rect::new(0, 0, 10, 6);
        let r = centered_rect(area, 40, 25);
        assert_eq!(r, area);
        assert!(area.union(r) == area, "{r:?} must stay inside {area:?}");
    }

    #[test]
    fn centered_x_matches_centered_rect() {
        let area = Rect::new(7, 0, 51, 40);
        assert_eq!(centered_x(area, 20), centered_rect(area, 20, 10).x);
    }

    // ---- scroll_offset ----------------------------------------------------

    #[test]
    fn scroll_offset_is_zero_when_everything_fits() {
        assert_eq!(scroll_offset(2, 3, 10), 0);
        assert_eq!(scroll_offset(9, 10, 10), 0);
    }

    #[test]
    fn scroll_offset_keeps_cursor_on_the_last_visible_row() {
        // 10 items, 3 visible, cursor at 4 → window is [2, 5).
        assert_eq!(scroll_offset(4, 10, 3), 2);
    }

    #[test]
    fn scroll_offset_never_scrolls_past_the_end() {
        // cursor at the last item: window is the final `visible` rows.
        assert_eq!(scroll_offset(9, 10, 3), 7);
        // even a cursor beyond the list clamps to the last window.
        assert_eq!(scroll_offset(99, 10, 3), 7);
    }

    #[test]
    fn scroll_offset_zero_visible_rows_is_zero() {
        assert_eq!(scroll_offset(5, 10, 0), 0);
    }

    #[test]
    fn feed_role_label_none_is_hidden() {
        assert_eq!(feed_role_label(FeedRole::None), None);
    }

    #[test]
    fn feed_role_label_shows_kebab_role() {
        assert_eq!(
            feed_role_label(FeedRole::ReviewsParent).as_deref(),
            Some("role:reviews-parent  ")
        );
        assert_eq!(
            feed_role_label(FeedRole::MyReviews).as_deref(),
            Some("role:my-reviews  ")
        );
        assert_eq!(
            feed_role_label(FeedRole::Cve).as_deref(),
            Some("role:cve  ")
        );
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(fair_truncate_segments(&[], 20, " › "), "");
    }

    #[test]
    fn single_segment_uses_plain_truncate() {
        assert_eq!(fair_truncate_segments(&["Hello"], 20, " › "), "Hello");
        assert_eq!(fair_truncate_segments(&["Hello World"], 4, " › "), "Hel…");
    }

    #[test]
    fn fits_whole_joins_unchanged() {
        let out = fair_truncate_segments(&["A", "B", "C"], 20, " › ");
        assert_eq!(out, "A › B › C");
    }

    #[test]
    fn equal_segments_split_evenly() {
        // budget 15, sep " › " x2 = 6, name_budget = 9, 3 segments → 3 chars each.
        let out = fair_truncate_segments(&["Alpha", "Bravo", "Charl"], 15, " › ");
        assert_eq!(out, "Al… › Br… › Ch…");
    }

    #[test]
    fn short_segment_donates_slack_to_long() {
        // "PR" is short; its slack goes to the long segment.
        // budget 21, seps=6, name_budget=15. Segments: "A"(1), "PR Reviews"(10), "Bots PR"(7).
        let out = fair_truncate_segments(&["A", "PR Reviews", "Bots PR"], 21, " › ");
        // "A" whole (1); remaining pool 14 across 2 long segments → 7 each.
        assert_eq!(out, "A › PR Rev… › Bots PR");
    }

    #[test]
    fn narrow_budget_falls_back_to_whole_truncate() {
        // name_budget < 2 * n → whole-string truncate, never a bare ellipsis segment.
        let out = fair_truncate_segments(&["Alpha", "Bravo", "Charlie"], 8, " › ");
        assert_eq!(out, "Alpha ›…");
        assert!(out.chars().count() <= 8);
    }

    #[test]
    fn empty_segments_never_exceed_budget() {
        let out = fair_truncate_segments(&["", ""], 1, " › ");
        assert!(out.chars().count() <= 1);
    }
}
