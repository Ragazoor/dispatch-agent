//! Top-level routing table for `App::update()`.
//!
//! This file only routes the outer [`Message`] enum to its per-domain inner
//! enum. The per-variant wiring (variant → `App::handle_*`) lives beside each
//! `*Message` enum in `messages/*.rs` as an inherent `route(self, app)` method
//! — see [`crate::tui::messages::SplitMessage::route`]. Adding a new domain
//! interaction is therefore a single-file edit (the variant plus its arm in
//! `messages/<domain>.rs`) plus its `update/*` handler; only a brand-new
//! *domain* enum needs a line here.
//!
//! The `App` state container and lifecycle methods live in `mod.rs`;
//! per-message handlers live in `update/*.rs`.

use crate::tui::types::{Command, Message};
use crate::tui::App;

/// Process a message and return a list of side-effect commands.
pub(in crate::tui) fn dispatch(app: &mut App, msg: Message) -> Vec<Command> {
    match msg {
        // ── Board navigation, view toggles, system events ──
        Message::System(sm) => sm.route(app),
        Message::Task(tm) => tm.route(app),
        Message::NavigateColumn(delta) => app.handle_navigate_column(delta),
        Message::NavigateRow(delta) => app.handle_navigate_row(delta),
        Message::NavigateRowFirst => app.handle_navigate_row_first(),
        Message::NavigateRowLast => app.handle_navigate_row_last(),
        Message::Split(sm) => sm.route(app),
        Message::RepoPathsUpdated(paths) => app.handle_repo_paths_updated(paths),
        Message::BaseBranchesUpdated(map) => app.handle_base_branches_updated(map),

        Message::ClearSelection => app.handle_clear_selection(),
        Message::SelectAllColumn => app.handle_select_all_column(),

        // ── Form input, text entry, creation flows ──
        Message::Input(im) => im.route(app),
        Message::Editor(em) => em.route(app),

        // ── Epic CRUD, lifecycle ──
        Message::Epic(em) => em.route(app),

        // ── PR flow: creation, merge, review state ──
        Message::Pr(pm) => pm.route(app),

        // ── Task repo filters and filter presets ──
        Message::RepoFilter(rfm) => rfm.route(app),

        // ── Local-first repo sync ──
        Message::RepoSync(rsm) => rsm.route(app),

        Message::Feed(fm) => fm.route(app),
        Message::Todo(tm) => tm.route(app),

        // ── Main session ──
        Message::MainSession(mm) => mm.route(app),

        // ── Budget indicator ──
        Message::Budget(bm) => bm.route(app),
    }
}
