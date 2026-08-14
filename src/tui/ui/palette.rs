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
// this ramp: column identity lives in the header bar, the card stripe,
// and the selected card's border. The ground is uniform across every
// column and sits *below* the bare terminal background (#1a1b26) so
// cards read as raised rather than inset.
pub(super) const BOARD_GROUND: Color = Color::Rgb(22, 22, 30); // #16161e
pub(super) const BOARD_GROUND_FOCUSED: Color = Color::Rgb(28, 28, 38); // #1c1c26
pub(super) const CARD_SURFACE: Color = Color::Rgb(36, 40, 59); // #24283b
// A resting card's frame. Neutral by design — the frame takes the
// column's identity colour only when the card is selected.
pub(super) const CARD_BORDER: Color = Color::Rgb(59, 66, 97); // #3b4261
