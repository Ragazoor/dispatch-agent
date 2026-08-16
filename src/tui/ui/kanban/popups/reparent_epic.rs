//! Reparent-epic tree picker overlay.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::Text,
    widgets::{Block, BorderType, Paragraph},
    Frame,
};
use std::collections::HashSet;

use crate::models::{Epic, EpicId};
use crate::tui::ui::palette::{MUTED, PURPLE};
use crate::tui::ui::shared::{centered_x, open_overlay, titled_block};
use crate::tui::{types::REPARENT_NO_PARENT_SENTINEL, App, InputMode};

pub(in crate::tui::ui::kanban) fn render_reparent_epic_overlay(
    frame: &mut Frame,
    app: &App,
    area: Rect,
) {
    let picker = match &app.interaction.reparent_picker {
        Some(p)
            if matches!(
                app.input.mode,
                InputMode::ReparentEpic(_) | InputMode::ConfirmReparentEpic { .. }
            ) =>
        {
            p
        }
        _ => return,
    };

    let title = app
        .board
        .epics
        .iter()
        .find(|e| e.id == picker.epic_id)
        .map(|e| format!(" Reparent epic: \"{}\" ", e.title))
        .unwrap_or_else(|| " Reparent epic ".to_string());

    render_tree_picker(frame, area, &title, &picker.items, &picker.tree_state);
}

pub(in crate::tui::ui::kanban) fn render_move_task_overlay(
    frame: &mut Frame,
    app: &App,
    area: Rect,
) {
    let picker = match &app.interaction.move_task_picker {
        Some(p)
            if matches!(
                app.input.mode,
                InputMode::MoveTaskToEpic(_) | InputMode::ConfirmMoveTaskToEpic { .. }
            ) =>
        {
            p
        }
        _ => return,
    };

    let title = app
        .board
        .tasks
        .iter()
        .find(|t| t.id == picker.task_id)
        .map(|t| format!(" Move task: \"{}\" ", t.title))
        .unwrap_or_else(|| " Move task to epic ".to_string());

    render_tree_picker(frame, area, &title, &picker.items, &picker.tree_state);
}

/// Shared magenta-bordered tree picker overlay used by both the reparent-epic
/// and move-task-to-epic flows: centered box, a hint footer, and a stateful
/// `tui_tree_widget::Tree` rendered from the pre-built `items`.
fn render_tree_picker(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: &[tui_tree_widget::TreeItem<'static, String>],
    tree_state: &std::cell::RefCell<tui_tree_widget::TreeState<String>>,
) {
    let overlay_width = (area.width * 50 / 100).clamp(30, 80);
    let overlay_height = area.height.saturating_sub(4).max(10);
    // Horizontally centred, but top-pinned two rows below the board — not
    // `centered_rect`, which centres on both axes.
    let overlay_area = Rect::new(
        centered_x(area, overlay_width),
        area.y + 2,
        overlay_width,
        overlay_height,
    );

    let block = titled_block(PURPLE, BorderType::Rounded, title.to_string());

    let inner_area = open_overlay(frame, overlay_area, block);

    // Split inner area: tree above, hint line at bottom
    let footer_area = Rect {
        y: inner_area.y + inner_area.height.saturating_sub(1),
        height: 1,
        ..inner_area
    };
    let tree_area = Rect {
        height: inner_area.height.saturating_sub(1),
        ..inner_area
    };

    let hints = Paragraph::new(
        " j/k:navigate  l/Space/\u{2192}:expand  h/\u{2190}:collapse  Enter:select  q/Esc:cancel",
    )
    .style(Style::default().fg(MUTED));
    frame.render_widget(hints, footer_area);

    let tree = match tui_tree_widget::Tree::new(items) {
        Ok(t) => t
            .block(Block::default())
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        Err(_) => return,
    };

    frame.render_stateful_widget(tree, tree_area, &mut tree_state.borrow_mut());
}

/// Build the reparent picker tree from the pre-filtered `eligible` epics.
///
/// Eligibility (target/descendant exclusion, status, board filter) is decided
/// by [`crate::tui::App::reparent_target_epics`]. Here we only assemble the
/// hierarchy. Because status filtering can drop a parent while keeping an
/// eligible child, any epic whose `parent_epic_id` is not itself eligible is
/// re-rooted to the top level so it stays selectable.
pub(in crate::tui) fn build_reparent_tree(
    eligible: &[&Epic],
) -> Vec<tui_tree_widget::TreeItem<'static, String>> {
    let no_parent = tui_tree_widget::TreeItem::new_leaf(
        REPARENT_NO_PARENT_SENTINEL.to_string(),
        Text::raw("— no parent —"),
    );
    let eligible_ids: HashSet<EpicId> = eligible.iter().map(|e| e.id).collect();
    let mut items = vec![no_parent];
    items.extend(build_epic_nodes(eligible, &eligible_ids, None));
    items
}

fn build_epic_nodes(
    epics: &[&Epic],
    eligible_ids: &HashSet<EpicId>,
    parent_id: Option<EpicId>,
) -> Vec<tui_tree_widget::TreeItem<'static, String>> {
    epics
        .iter()
        .filter(|e| match parent_id {
            // Top level: epics with no parent, or whose parent was filtered out
            // (re-rooted orphans).
            None => e.parent_epic_id.is_none_or(|p| !eligible_ids.contains(&p)),
            Some(pid) => e.parent_epic_id == Some(pid),
        })
        .filter_map(|e| {
            let children = build_epic_nodes(epics, eligible_ids, Some(e.id));
            let id = format!("epic:{}", e.id.0);
            if children.is_empty() {
                Some(tui_tree_widget::TreeItem::new_leaf(
                    id,
                    Text::from(e.title.clone()),
                ))
            } else {
                tui_tree_widget::TreeItem::new(id, Text::from(e.title.clone()), children).ok()
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::models::TaskStatus;

    fn epic(id: i64, parent: Option<i64>) -> Epic {
        let now = chrono::Utc::now();
        Epic {
            id: EpicId(id),
            title: format!("Epic {id}"),
            description: String::new(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: false,
            parent_epic_id: parent.map(EpicId),
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: now,
            updated_at: now,
        }
    }

    /// Top-level identifiers in the built tree, in order, excluding the
    /// "— no parent —" sentinel.
    fn root_ids(items: &[tui_tree_widget::TreeItem<'static, String>]) -> Vec<String> {
        items
            .iter()
            .map(|i| i.identifier().clone())
            .filter(|id| id != REPARENT_NO_PARENT_SENTINEL)
            .collect()
    }

    #[test]
    fn first_item_is_no_parent_sentinel() {
        let items = build_reparent_tree(&[]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].identifier(), REPARENT_NO_PARENT_SENTINEL);
    }

    #[test]
    fn nested_epics_render_under_their_parent() {
        let e1 = epic(1, None);
        let e2 = epic(2, Some(1));
        let eligible = [&e1, &e2];
        let items = build_reparent_tree(&eligible);
        // Only epic 1 is at the top level; epic 2 is nested under it.
        assert_eq!(root_ids(&items), vec!["epic:1"]);
    }

    #[test]
    fn orphaned_child_is_rerooted_when_parent_filtered_out() {
        // Parent epic 1 is NOT eligible (e.g. it was Done); child epic 2 is.
        // Epic 2 must still appear, re-rooted to the top level.
        let e2 = epic(2, Some(1));
        let eligible = [&e2];
        let items = build_reparent_tree(&eligible);
        assert_eq!(root_ids(&items), vec!["epic:2"]);
    }
}
