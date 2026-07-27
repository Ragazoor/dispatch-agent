use ratatui::style::Color;

// ── Tokyo Night palette ─────────────────────────────────────────────
pub(super) const MUTED: Color = Color::Rgb(86, 95, 137);
pub(super) const MUTED_LIGHT: Color = Color::Rgb(120, 124, 153);
pub(crate) const FG: Color = Color::Rgb(192, 202, 245);
pub(super) const BORDER: Color = Color::Rgb(41, 46, 66);
pub(crate) const YELLOW: Color = Color::Rgb(224, 175, 104);
pub(super) const PURPLE: Color = Color::Rgb(187, 154, 247);
pub(super) const GREEN: Color = Color::Rgb(158, 206, 106);
pub(super) const CYAN: Color = Color::Rgb(86, 182, 194);
pub(crate) const BLUE: Color = Color::Rgb(122, 162, 247);
pub(super) const FLASH_BG: Color = Color::Rgb(62, 52, 20);

pub(super) const RED: Color = Color::Rgb(247, 118, 142);

// Archive column — muted blue-gray stripe
pub(super) const ARCHIVE_STRIPE: Color = Color::Rgb(72, 82, 120);
// Archive column background tint
pub(super) const ARCHIVE_COL_BG: Color = Color::Rgb(26, 28, 40);
