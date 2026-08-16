//! Help overlay.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{BorderType, Paragraph},
    Frame,
};

use crate::tui::ui::palette::CYAN;
use crate::tui::ui::shared::{centered_rect, open_overlay, titled_block, HintStyles};
use crate::tui::{App, InputMode};

pub(in crate::tui::ui::kanban) fn render_help_overlay(frame: &mut Frame, app: &App, area: Rect) {
    if app.input.mode != InputMode::Help {
        return;
    }

    let popup_width = (area.width * 80 / 100).clamp(40, 72);
    let popup_height = (area.height * 80 / 100).clamp(25, 36);
    let popup_area = centered_rect(area, popup_width, popup_height);

    let block = titled_block(CYAN, BorderType::Double, " Help ".to_string());

    let styles = HintStyles::new(CYAN);
    // Section headers and key glyphs share the accent; kept as local aliases so
    // the table below stays readable.
    let (header, key, desc, note) = (styles.accent, styles.accent, styles.desc, styles.note);

    // Keep this body short enough that the `General` section still renders at
    // the popup's clamped 25-row floor (inner height = popup_height - 2, so 23
    // lines are visible there). `render_help_overlay_fits_the_clamped_floor`
    // pins that; adding a line without removing one will fail it.
    let lines = vec![
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
            Span::styled("  [gg/[]", key),
            Span::styled(" top   ", desc),
            Span::styled("[G/]]", key),
            Span::styled(" bottom   ", desc),
            Span::styled("[Enter]", key),
            Span::styled(" task detail", desc),
        ]),
        Line::from(vec![
            Span::styled("  [q]", key),
            Span::styled(" quit / exit epic   ", desc),
            Span::styled("[Esc]", key),
            Span::styled(" clear selection", desc),
        ]),
        Line::from(""),
        Line::from(Span::styled("  Actions", header)),
        Line::from(vec![
            Span::styled("  [n]", key),
            Span::styled(" new task   ", desc),
            Span::styled("[c]", key),
            Span::styled(" copy   ", desc),
            Span::styled("[e]", key),
            Span::styled(" edit / enter epic", desc),
        ]),
        Line::from(vec![
            Span::styled("  [E]", key),
            Span::styled(" new epic   ", desc),
            Span::styled("[m]", key),
            Span::styled(" move task to epic / reparent", desc),
        ]),
        Line::from(vec![
            Span::styled("  [Space]", key),
            Span::styled(" dispatch / resume / jump to agent*", desc),
        ]),
        Line::from(vec![
            Span::styled("  [H/L]", key),
            Span::styled(" move back/forward   ", desc),
            Span::styled("[J/K]", key),
            Span::styled(" reorder item", desc),
        ]),
        Line::from(vec![
            Span::styled("  [x]", key),
            Span::styled(" done / archive   ", desc),
            Span::styled("[D]", key),
            Span::styled(" quick dispatch", desc),
        ]),
        Line::from(vec![
            Span::styled("  [v]", key),
            Span::styled(" select   ", desc),
            Span::styled("[a]", key),
            Span::styled(" select all   ", desc),
            Span::styled("[/]", key),
            Span::styled(" search titles/ids", desc),
        ]),
        Line::from(vec![
            Span::styled("  [f]", key),
            Span::styled(" filter repos   ", desc),
            Span::styled("[A]", key),
            Span::styled(" active only   ", desc),
            Span::styled("[F]", key),
            Span::styled(" flat view", desc),
        ]),
        Line::from(vec![
            Span::styled("  [p]", key),
            Span::styled(" open PR   ", desc),
            Span::styled("[:]", key),
            Span::styled(" main session   ", desc),
            Span::styled("[o]", key),
            Span::styled(" sync repo", desc),
        ]),
        Line::from(vec![
            Span::styled("  [s]", key),
            Span::styled(" toggle split (then [Space] swaps into pane)   ", desc),
            Span::styled("[T]", key),
            Span::styled(" detach", desc),
        ]),
        Line::from(vec![
            Span::styled("  [r]", key),
            Span::styled(" refresh feed   ", desc),
            Span::styled("[U]", key),
            Span::styled(" auto-dispatch   ", desc),
            Span::styled("[R]", key),
            Span::styled(" by repo", desc),
        ]),
        Line::from(vec![
            Span::styled("  [P]", key),
            Span::styled(" todos   ", desc),
            Span::styled("[t]", key),
            Span::styled(" add todo from card   ", desc),
            Span::styled("[N]", key),
            Span::styled(" notifications", desc),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  * [Space] jumps to the agent's window if one is live,",
            note,
        )),
        Line::from(Span::styled(
            "    or swaps into the split pane; else dispatch/resume; epic: enter",
            note,
        )),
        Line::from(""),
        Line::from(Span::styled("  General", header)),
        Line::from(vec![
            Span::styled("  [?]", key),
            Span::styled(" this help   ", desc),
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
        Line::from(Span::styled("  [?] or [Esc] to close", note)),
    ];

    let inner = open_overlay(frame, popup_area, block);
    frame.render_widget(Paragraph::new(lines), inner);
}
