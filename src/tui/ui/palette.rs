use ratatui::style::Color;

// ── Tokyo Night palette ─────────────────────────────────────────────
pub(crate) const MUTED: Color = Color::Rgb(86, 95, 137);
pub(super) const MUTED_LIGHT: Color = Color::Rgb(120, 124, 153);
pub(crate) const FG: Color = Color::Rgb(192, 202, 245);
pub(crate) const BORDER: Color = Color::Rgb(41, 46, 66);
pub(crate) const YELLOW: Color = Color::Rgb(224, 175, 104);
pub(crate) const PURPLE: Color = Color::Rgb(187, 154, 247);
pub(crate) const GREEN: Color = Color::Rgb(158, 206, 106);
pub(crate) const CYAN: Color = Color::Rgb(86, 182, 194);
pub(crate) const BLUE: Color = Color::Rgb(122, 162, 247);
pub(crate) const RED: Color = Color::Rgb(247, 118, 142);
pub(crate) const FLASH_BG: Color = Color::Rgb(62, 52, 20);

// Archive column — muted blue-gray stripe
pub(crate) const ARCHIVE_STRIPE: Color = Color::Rgb(72, 82, 120);

// ── Board neutral ramp (core.allium: BoardNeutralRamp) ──────────────
// Four neutral surfaces in strictly ascending lightness. No hue enters
// this ramp: column identity lives in the header label and the card
// stripe. (The card frame carries state, not identity — see
// CURSOR_BORDER.) The ground is uniform across every
// column and sits *below* the bare terminal background (#1a1b26) so
// cards read as raised rather than inset.
pub(super) const BOARD_GROUND: Color = Color::Rgb(22, 22, 30); // #16161e
pub(super) const BOARD_GROUND_FOCUSED: Color = Color::Rgb(28, 28, 38); // #1c1c26
pub(super) const CARD_SURFACE: Color = Color::Rgb(36, 40, 59); // #24283b
// A resting card's frame. Neutral by design — the frame carries state,
// and a healthy card has none to report.
pub(super) const CARD_BORDER: Color = Color::Rgb(59, 66, 97); // #3b4261

// The selected card's frame. A near-white owned by nothing else on the
// board, deliberately outside the hue vocabulary so the cursor never
// competes with the state colours it sits among — and a step brighter
// than FG so it does not read as a stray line of ordinary card text
// (`core.allium`: "Selection").
pub(super) const CURSOR_BORDER: Color = Color::Rgb(232, 237, 251); // #e8edfb

// ── Column header bar (core.allium: "Column header bar") ────────────
// The header fill carries no hue and is uniform across every column,
// like the ground. Identity lives in the *label*, which keeps its hue at
// both focus states while only its brightness moves. Focus raises the
// fill's lightness neutrally.
pub(super) const HEADER_BG: Color = Color::Rgb(26, 26, 36); // #1a1a24
pub(super) const HEADER_BG_FOCUSED: Color = Color::Rgb(34, 34, 46); // #22222e

// Fill behind the focused column's select-all checkbox. One neutral
// value, not a per-column ramp: with no hued fills anywhere on the board
// a hued checkbox would be the only one. Same value as `CARD_BORDER`,
// aliased rather than duplicated so the shared literal has one home while
// the two roles stay separately named.
pub(super) const SELECT_ALL_HIGHLIGHT_BG: Color = CARD_BORDER;

const WHITE: Color = Color::Rgb(255, 255, 255);

/// One channel of [`mix`], rounded rather than truncated.
///
/// The `+ 50` is what makes this round to nearest instead of toward zero; a
/// truncating blend shifts derived colours a point darker per channel, which is
/// visible on the header labels.
const fn blend(a: u8, b: u8, pct: u16) -> u8 {
    (((a as u16) * (100 - pct) + (b as u16) * pct + 50) / 100) as u8
}

/// Linear blend of two palette colours: `pct`% of the way from `a` to `b`.
///
/// A `const fn` on purpose. Derived colours — the header labels being the chief
/// case — are then computed from their source hue at compile time rather than
/// pasted in as literals, so changing a hue in `column_color` moves everything
/// derived from it instead of silently leaving stale values behind that no test
/// can catch.
///
/// Panics at const-evaluation time (i.e. fails the build) on a non-`Rgb` input.
/// Every colour in this palette is `Rgb`, so that arm is unreachable; making it a
/// compile error rather than a silent fallback is what keeps it unreachable.
pub(super) const fn mix(a: Color, b: Color, pct: u16) -> Color {
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            Color::Rgb(blend(ar, br, pct), blend(ag, bg, pct), blend(ab, bb, pct))
        }
        _ => panic!("palette colours must be Rgb to be mixed"),
    }
}

/// A hue dimmed toward the header fill — an unfocused column's header label.
pub(super) const fn header_label_unfocused(hue: Color) -> Color {
    mix(hue, HEADER_BG, 30)
}

/// A hue brightened toward white — a focused column's header label.
pub(super) const fn header_label_focused(hue: Color) -> Color {
    mix(hue, WHITE, 25)
}
