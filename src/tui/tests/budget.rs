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
