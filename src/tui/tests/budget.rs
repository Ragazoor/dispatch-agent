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
        crate::tui::tests::helpers::buffer_line(&buf, 0)
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

    /// The same app, switched into epic view on an epic that lights up every
    /// epic-only badge: `auto dispatch [U]  ` (19), `role:my-reviews  ` (17)
    /// and `group:on [R]  ` (14), on top of `[1/2 repos]  ` (13) and the bell
    /// (5 codepoints, 6 display columns). That is the widest the top row ever
    /// gets — 68 codepoints of pre-existing badges, so `render_top_indicators`
    /// hands the budget span only `width - 69` (68 plus the emoji-undercount
    /// reserve).
    ///
    /// The `append-only` marker (epics.allium: AppendOnlyEpicIndicator) cannot
    /// widen this. The write path refuses append-only together with grouping
    /// or a feed role (epics.allium: UpdateEpicViaMcp), so the marker's widest
    /// row is `manual dispatch [U]  append-only  group:off [R]  ` — 49
    /// codepoints against the 50 this one spends on its three epic badges. A
    /// refused `R` press can flash `append-only  group:on [R]` before the
    /// rollback lands (ToggleGroupByRepo), and even that is only 48.
    fn epic_view_app_with_every_badge_and_budget() -> crate::tui::App {
        use crate::models::{EpicId, FeedRole};
        use crate::tui::types::{BoardSelection, ViewMode};

        let mut app = app_with_badges_and_budget();
        let mut epic = crate::tui::tests::helpers::make_epic(10);
        epic.auto_dispatch = true;
        epic.group_by_repo = true;
        epic.feed_role = FeedRole::MyReviews;
        app.board.epics = vec![epic];
        app.board.view_mode = ViewMode::Epic {
            epic_id: EpicId(10),
            selection: BoardSelection::new_for_epic(),
            parent: Box::new(ViewMode::Board(BoardSelection::new())),
        };
        // board.epics was mutated directly, bypassing the message system.
        app.invalidate_layout_cache();
        app
    }

    /// Epic view is the case the design called for alongside board view, and the
    /// one most likely to squeeze the budget span: at width 88 the budget gets
    /// 19 columns, so the full two-window form (27, with countdowns) cannot fit
    /// but the countdown-less form (16) can. Every epic badge must survive.
    #[test]
    fn epic_view_badges_squeeze_the_budget_but_are_never_dropped() {
        let mut app = epic_view_app_with_every_badge_and_budget();
        let row = top_row(&mut app, 88);
        assert!(row.contains("auto dispatch [U]"), "got {row:?}");
        assert!(row.contains("role:my-reviews"), "got {row:?}");
        assert!(row.contains("group:on [R]"), "got {row:?}");
        assert!(row.contains("[1/2 repos]"), "got {row:?}");
        assert!(
            row.trim_end().ends_with("[N]"),
            "bell badge must render intact, not truncated to '[N': {row:?}"
        );
        assert!(row.contains("5h 23%"), "got {row:?}");
        assert!(row.contains("7d 41%"), "got {row:?}");
        assert!(
            !row.contains('\u{00B7}'),
            "countdowns should have been dropped to fit: {row:?}"
        );
    }

    /// At width 72 the budget span gets 3 columns — no degradation level fits,
    /// so it must vanish entirely rather than push an epic badge off-screen.
    #[test]
    fn very_narrow_epic_view_drops_budget_entirely_but_keeps_epic_badges() {
        let mut app = epic_view_app_with_every_badge_and_budget();
        let row = top_row(&mut app, 72);
        assert!(!row.contains("5h"), "budget badge must be gone: {row:?}");
        assert!(!row.contains("7d"), "budget badge must be gone: {row:?}");
        assert!(row.contains("auto dispatch [U]"), "got {row:?}");
        assert!(row.contains("role:my-reviews"), "got {row:?}");
        assert!(row.contains("group:on [R]"), "got {row:?}");
        assert!(row.contains("[1/2 repos]"), "got {row:?}");
        assert!(
            row.trim_end().ends_with("[N]"),
            "bell badge must render intact: {row:?}"
        );
    }

    /// Regression test for the emoji-width undercount (dispatch.allium:
    /// `@guarantee DegradesWhenRowTooNarrow` — pre-existing badges must never
    /// be clipped). `render_top_indicators` sums `used_width` in *codepoints*
    /// (`"\u{1F514} [N]"` is 5 codepoints) but ratatui measures the bell
    /// badge at 6 *display columns* (the emoji is double-width). At the exact
    /// width where the budget text's degraded form fills the miscounted
    /// budget precisely, the composed line is one column wider than `area`,
    /// and `Alignment::Right` truncates the right edge — clipping `[N]` down
    /// to `[N`. width=32 with no repo filter (used_width=5, real width=6) and
    /// the two-window+countdown budget text (27 columns incl. trailing
    /// spaces) reproduces exactly that: 32 - 5 == 27 "fits", but the real
    /// total is 33.
    #[test]
    fn narrow_width_never_clips_the_bell_badge() {
        let now = chrono::Utc::now().timestamp();
        let mut app = make_app();
        app.set_notifications_enabled(true);
        app.budget = Some(BudgetSnapshot {
            five_hour: Some(BudgetWindow {
                used_percentage: 23.4,
                resets_at: now + 8070,
            }),
            seven_day: Some(BudgetWindow {
                used_percentage: 41.2,
                resets_at: now + 349_200,
            }),
            captured_at: now,
        });

        let row = top_row(&mut app, 32);

        assert!(
            row.contains('\u{1F514}'),
            "bell badge must not be dropped: {row:?}"
        );
        assert!(
            row.trim_end().ends_with("[N]"),
            "bell badge must render intact, not truncated to '[N': {row:?}"
        );
    }
}
