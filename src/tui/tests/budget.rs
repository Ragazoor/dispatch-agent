#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::models::budget::{BudgetSnapshot, BudgetWindow};
use crate::tui::commands::BudgetCommand;
use crate::tui::tests::helpers::make_app;
use crate::tui::types::Command;

fn snapshot(pct: f64) -> BudgetSnapshot {
    BudgetSnapshot {
        five_hour: Some(BudgetWindow {
            used_percentage: pct,
            resets_at: 0,
        }),
        seven_day: None,
        captured_at: 0,
    }
}

#[test]
fn tick_emits_refresh_on_the_nth_tick() {
    let mut app = make_app();
    for _ in 0..(crate::tui::BUDGET_POLL_TICKS - 1) {
        let cmds = app.handle_tick();
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::Budget(BudgetCommand::Refresh))),
            "must not poll before the Nth tick"
        );
    }
    let cmds = app.handle_tick();
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::Budget(BudgetCommand::Refresh))),
        "must poll on the Nth tick"
    );
}

#[test]
fn changed_snapshot_marks_dirty() {
    let mut app = make_app();
    app.dirty = false;
    app.handle_budget_updated(Some(snapshot(10.0)));
    assert!(app.dirty, "a changed snapshot must force a redraw");
}

#[test]
fn unchanged_snapshot_does_not_mark_dirty() {
    // This state is invisible to the discriminant-based dirty detector in
    // handle_key, so the handler marks dirty itself — but only on change.
    let mut app = make_app();
    app.handle_budget_updated(Some(snapshot(10.0)));
    app.dirty = false;
    app.handle_budget_updated(Some(snapshot(10.0)));
    assert!(!app.dirty, "an identical refresh must not force a redraw");
}

#[test]
fn disappearing_snapshot_marks_dirty() {
    let mut app = make_app();
    app.handle_budget_updated(Some(snapshot(10.0)));
    app.dirty = false;
    app.handle_budget_updated(None);
    assert!(app.dirty);
}

#[test]
fn repeated_absent_snapshot_does_not_mark_dirty() {
    let mut app = make_app();
    app.handle_budget_updated(None);
    app.dirty = false;
    app.handle_budget_updated(None);
    assert!(!app.dirty);
}

#[test]
fn budget_stale_after_is_ten_minutes() {
    // Not consumed until Task 7 (rendering), but pinned here so the constant
    // isn't flagged dead by clippy in the meantime and its value is locked
    // down against accidental drift.
    assert_eq!(
        crate::tui::BUDGET_STALE_AFTER,
        std::time::Duration::from_secs(600)
    );
}

/// The render *glue* in `render_top_indicators` (`src/tui/ui/shared.rs`) —
/// computing `used_width` over the pre-existing badges, `saturating_sub`ing
/// it from `area.width`, calling `budget_spans`, and splicing the result at
/// index 0 — has no automated coverage from `budget_spans`'s own unit tests
/// (those drive the pure function directly) or from the 120-column snapshots
/// (wide enough that degradation never triggers). These tests render the
/// real `App` through the public `render` entry point (via `render_to_buffer`)
/// into deliberately narrow buffers, so a wrong width sum, a wrong splice
/// index, or a future edit that passes the full `area.width` instead of the
/// remainder would show up here instead of only in a narrow terminal.
mod render_glue {
    use super::*;
    use crate::tui::tests::helpers::render_to_buffer;
    use std::collections::HashSet;

    /// Row 0 of the rendered board is the top indicator bar. Read it back as
    /// a plain string so assertions can check substring presence/absence.
    fn top_row(app: &mut crate::tui::App, width: u16) -> String {
        let buf = render_to_buffer(app, width, 40);
        let area = buf.area();
        let mut line = String::new();
        for x in area.left()..area.right() {
            line.push_str(buf[(x, 0)].symbol());
        }
        line
    }

    /// An app with two pre-existing top-row badges (`[1/2 repos]` from an
    /// active repo filter, and the notification bell) plus a fresh two-window
    /// budget snapshot. `captured_at` is real `Utc::now()` since
    /// `render_top_indicators` reads the wall clock for `now`.
    fn app_with_badges_and_budget() -> crate::tui::App {
        let now = chrono::Utc::now().timestamp();
        let mut app = make_app();
        app.board.repo_paths = vec!["/repo/alpha".to_string(), "/repo/beta".to_string()];
        app.set_repo_filter(HashSet::from(["/repo/alpha".to_string()]));
        app.set_notifications_enabled(true);
        app.budget = Some(BudgetSnapshot {
            five_hour: Some(BudgetWindow {
                used_percentage: 23.4,
                resets_at: now + 8040,
            }),
            seven_day: Some(BudgetWindow {
                used_percentage: 41.2,
                resets_at: now + 345_600,
            }),
            captured_at: now,
        });
        app
    }

    #[test]
    fn wide_enough_shows_full_budget_alongside_existing_badges() {
        let mut app = app_with_badges_and_budget();
        let row = top_row(&mut app, 120);
        assert!(row.contains("5h 23%"), "got {row:?}");
        assert!(row.contains("7d 41%"), "got {row:?}");
        assert!(row.contains('\u{00B7}'), "expected a countdown: {row:?}");
        assert!(row.contains("[1/2 repos]"), "got {row:?}");
        assert!(row.contains('\u{1F514}'), "expected the bell: {row:?}");
    }

    #[test]
    fn narrow_degrades_budget_but_keeps_existing_badges_intact() {
        // At width 40 the full badge (with countdowns) no longer fits, so it
        // must degrade — but the repo-filter and bell badges, which existed
        // before the budget badge was added, must be untouched.
        let mut app = app_with_badges_and_budget();
        let row = top_row(&mut app, 40);
        assert!(row.contains("5h 23%"), "got {row:?}");
        assert!(row.contains("7d 41%"), "got {row:?}");
        assert!(
            !row.contains('\u{00B7}'),
            "countdowns should have been dropped to fit: {row:?}"
        );
        assert!(
            row.contains("[1/2 repos]"),
            "pre-existing badge must survive degradation: {row:?}"
        );
        assert!(
            row.contains('\u{1F514}'),
            "pre-existing bell badge must survive degradation: {row:?}"
        );
    }

    #[test]
    fn very_narrow_drops_budget_entirely_but_keeps_existing_badges_intact() {
        // At width 20 there is no room for the budget badge at any
        // degradation level, so it must disappear completely — while the
        // pre-existing badges are still fully rendered, not pushed off-screen.
        let mut app = app_with_badges_and_budget();
        let row = top_row(&mut app, 20);
        assert!(!row.contains("5h"), "budget badge must be gone: {row:?}");
        assert!(!row.contains("7d"), "budget badge must be gone: {row:?}");
        assert!(
            !row.contains('\u{00B7}'),
            "budget badge must be gone: {row:?}"
        );
        assert!(
            row.contains("[1/2 repos]"),
            "pre-existing badge must survive: {row:?}"
        );
        assert!(
            row.contains('\u{1F514}'),
            "pre-existing bell badge must survive: {row:?}"
        );
    }
}
