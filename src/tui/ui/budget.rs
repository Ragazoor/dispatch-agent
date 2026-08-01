//! Top-row budget indicator rendering (docs/specs/dispatch.allium:
//! TokenBudgetIndicator).
//!
//! `now` and `stale_after` are parameters rather than wall-clock reads so every
//! state is testable without sleeping (docs/conventions.md: no sleeping in
//! tests).

use super::palette::MUTED;
use crate::models::budget::{BudgetSnapshot, BudgetWindow};
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use std::time::Duration;

/// Colour for a used-percentage: comfortable, tightening, nearly gone.
fn window_style(pct: f64) -> Style {
    let colour = if pct > 80.0 {
        Color::Red
    } else if pct >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    };
    Style::default().fg(colour)
}

/// Compact countdown. Never negative: a reset already in the past reads "now".
fn format_countdown(seconds_remaining: i64) -> String {
    if seconds_remaining <= 0 {
        return "now".to_string();
    }
    let days = seconds_remaining / 86_400;
    if days > 0 {
        return format!("{days}d");
    }
    let hours = seconds_remaining / 3_600;
    let minutes = (seconds_remaining % 3_600) / 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

fn window_text(label: &str, window: &BudgetWindow, now: i64, with_countdown: bool) -> String {
    let pct = window.clamped_percentage();
    if with_countdown {
        format!(
            "{label} {pct:.0}% \u{00B7}{}",
            format_countdown(window.resets_at - now)
        )
    } else {
        format!("{label} {pct:.0}%")
    }
}

/// Build the indicator's spans, degrading to fit `width_budget`.
///
/// Degradation order, per the spec: drop the countdown suffixes, then the
/// seven-day window, then the indicator entirely. Pre-existing badges in the row
/// are never sacrificed to make room for this one — it is the newest and least
/// critical occupant.
pub(in crate::tui::ui) fn budget_spans(
    snapshot: Option<&BudgetSnapshot>,
    now: i64,
    stale_after: Duration,
    width_budget: usize,
) -> Vec<Span<'static>> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    if snapshot.five_hour.is_none() && snapshot.seven_day.is_none() {
        return Vec::new();
    }

    let age = now.saturating_sub(snapshot.captured_at).max(0);
    let stale = age as u64 > stale_after.as_secs();
    let age_suffix = if stale {
        format!(" ({}m old)", age / 60)
    } else {
        String::new()
    };

    // Try each degradation level in order and take the first that fits.
    for (with_countdown, with_seven_day) in [(true, true), (false, true), (false, false)] {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if let Some(w) = snapshot.five_hour.as_ref() {
            let style = if stale {
                Style::default().fg(MUTED)
            } else {
                window_style(w.clamped_percentage())
            };
            spans.push(Span::styled(
                window_text("5h", w, now, with_countdown),
                style,
            ));
        }
        if with_seven_day {
            if let Some(w) = snapshot.seven_day.as_ref() {
                if !spans.is_empty() {
                    spans.push(Span::raw("  "));
                }
                let style = if stale {
                    Style::default().fg(MUTED)
                } else {
                    window_style(w.clamped_percentage())
                };
                spans.push(Span::styled(
                    window_text("7d", w, now, with_countdown),
                    style,
                ));
            }
        }
        if spans.is_empty() {
            return Vec::new();
        }
        if !age_suffix.is_empty() {
            spans.push(Span::styled(age_suffix.clone(), Style::default().fg(MUTED)));
        }
        spans.push(Span::raw("  "));

        let width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if width <= width_budget {
            return spans;
        }
    }
    Vec::new()
}

// `budget_spans` is `pub(in crate::tui::ui)` — narrower than the
// `pub(in crate::tui)` visibility `src/tui/tests/` relies on for other
// render helpers (see `action_hints`/`column_color` re-exports in
// `src/tui/ui/mod.rs`), and a re-export cannot broaden an item's original
// visibility. So these tests live here, inline, rather than in
// `src/tui/tests/budget.rs`.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const STALE: Duration = Duration::from_secs(600);
    const WIDE: usize = 200;

    fn full(five: f64, seven: f64, captured_at: i64) -> BudgetSnapshot {
        BudgetSnapshot {
            five_hour: Some(BudgetWindow {
                used_percentage: five,
                resets_at: captured_at + 8040,
            }),
            seven_day: Some(BudgetWindow {
                used_percentage: seven,
                resets_at: captured_at + 345_600,
            }),
            captured_at,
        }
    }

    fn text_of(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn renders_both_windows_with_percent_and_countdown() {
        let snap = full(23.4, 41.2, 1000);
        let text = text_of(&budget_spans(Some(&snap), 1000, STALE, WIDE));
        assert!(text.contains("5h 23%"), "got {text:?}");
        assert!(text.contains("7d 41%"), "got {text:?}");
        assert!(text.contains("2h14m"), "got {text:?}");
        assert!(text.contains("4d"), "got {text:?}");
    }

    #[test]
    fn no_snapshot_renders_nothing() {
        assert!(budget_spans(None, 0, STALE, WIDE).is_empty());
    }

    #[test]
    fn omits_absent_window() {
        let snap = BudgetSnapshot {
            five_hour: Some(BudgetWindow {
                used_percentage: 5.0,
                resets_at: 60,
            }),
            seven_day: None,
            captured_at: 0,
        };
        let text = text_of(&budget_spans(Some(&snap), 0, STALE, WIDE));
        assert!(text.contains("5h"));
        assert!(!text.contains("7d"));
    }

    #[test]
    fn colours_by_threshold() {
        let green = budget_spans(Some(&full(10.0, 10.0, 0)), 0, STALE, WIDE);
        let yellow = budget_spans(Some(&full(65.0, 65.0, 0)), 0, STALE, WIDE);
        let red = budget_spans(Some(&full(91.0, 91.0, 0)), 0, STALE, WIDE);
        assert!(green.iter().any(|s| s.style.fg == Some(Color::Green)));
        assert!(yellow.iter().any(|s| s.style.fg == Some(Color::Yellow)));
        assert!(red.iter().any(|s| s.style.fg == Some(Color::Red)));
    }

    #[test]
    fn reset_in_the_past_renders_now_not_a_negative_countdown() {
        let snap = BudgetSnapshot {
            five_hour: Some(BudgetWindow {
                used_percentage: 5.0,
                resets_at: 100,
            }),
            seven_day: None,
            captured_at: 100,
        };
        let text = text_of(&budget_spans(Some(&snap), 500, STALE, WIDE));
        assert!(text.contains("now"), "got {text:?}");
        assert!(
            !text.contains('-'),
            "must never render a negative countdown: {text:?}"
        );
    }

    #[test]
    fn clamps_out_of_range_percentage() {
        let snap = BudgetSnapshot {
            five_hour: Some(BudgetWindow {
                used_percentage: 231.0,
                resets_at: 0,
            }),
            seven_day: Some(BudgetWindow {
                used_percentage: -9.0,
                resets_at: 0,
            }),
            captured_at: 0,
        };
        let text = text_of(&budget_spans(Some(&snap), 0, STALE, WIDE));
        assert!(text.contains("5h 100%"), "got {text:?}");
        assert!(text.contains("7d 0%"), "got {text:?}");
    }

    #[test]
    fn stale_snapshot_is_dimmed_and_shows_age() {
        let snap = full(23.0, 41.0, 0);
        let text = text_of(&budget_spans(Some(&snap), 1_020, STALE, WIDE));
        assert!(text.contains("17m old"), "got {text:?}");
    }

    #[test]
    fn fresh_snapshot_shows_no_age_suffix() {
        let snap = full(23.0, 41.0, 0);
        let text = text_of(&budget_spans(Some(&snap), 60, STALE, WIDE));
        assert!(!text.contains("old"), "got {text:?}");
    }

    #[test]
    fn degrades_by_dropping_countdowns_first() {
        let snap = full(23.0, 41.0, 0);
        let text = text_of(&budget_spans(Some(&snap), 0, STALE, 18));
        assert!(
            text.contains("5h 23%") && text.contains("7d 41%"),
            "got {text:?}"
        );
        assert!(
            !text.contains('\u{00B7}'),
            "countdowns must be dropped first: {text:?}"
        );
    }

    #[test]
    fn degrades_by_dropping_seven_day_next() {
        let snap = full(23.0, 41.0, 0);
        let text = text_of(&budget_spans(Some(&snap), 0, STALE, 10));
        assert!(text.contains("5h 23%"), "got {text:?}");
        assert!(!text.contains("7d"), "got {text:?}");
    }

    #[test]
    fn degrades_to_nothing_when_hopeless() {
        let snap = full(23.0, 41.0, 0);
        assert!(budget_spans(Some(&snap), 0, STALE, 3).is_empty());
    }
}
