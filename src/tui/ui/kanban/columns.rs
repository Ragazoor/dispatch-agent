//! Kanban column layout and per-column rendering.

use chrono::{DateTime, Utc};
use ratatui::{
    layout::{Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

use crate::models::{EpicId, TaskStatus};
use crate::tui::{App, ColumnItem, ColumnLayout, EpicStatsMap, ViewMode};

use super::super::palette::{MUTED, PURPLE};
use super::super::shared::{render_substatus_header, rounded_block, truncate};
use super::cards::{build_task_list_item, render_epic_header_item, render_epic_item, ColRenderCtx};
use super::{board_column_constraints, column_bg_color, column_color, render_column_separator};

fn render_orphan_separator(col_width: u16, is_first: bool) -> ListItem<'static> {
    // ╌╌ · ╌╌╌╌╌╌╌╌╌╌╌ — dashed rule with centre dot, all muted.
    // Visually distinct from: epic headers (purple + named), card rules (solid ─).
    const DASH: &str = "\u{254C}"; // ╌ BOX DRAWINGS LIGHT DOUBLE DASH HORIZONTAL
    let prefix = format!("{} \u{00B7} ", DASH.repeat(2)); // "╌╌ · "
    let prefix_chars = prefix.chars().count();
    let fill_count = (col_width as usize).saturating_sub(prefix_chars);
    let line = Line::from(Span::styled(
        format!("{}{}", prefix, DASH.repeat(fill_count)),
        Style::default().fg(MUTED),
    ));
    if is_first {
        ListItem::new(vec![line])
    } else {
        ListItem::new(vec![Line::raw(""), line])
    }
}

// ---------------------------------------------------------------------------
// Pre-computed column render data
// ---------------------------------------------------------------------------

/// Pre-built rendering data for one task-status column.
/// Produced during the immutable phase; consumed by the mutable render phase.
struct TaskColData {
    list_items: Vec<ListItem<'static>>,
    list_selection_idx: Option<usize>,
    item_heights: Vec<usize>,
    col_area: Rect,
    is_focused: bool,
    status: TaskStatus,
    color: Color,
}

/// Pre-built rendering data for the archive column.
struct ArchiveColData {
    items: Vec<ListItem<'static>>,
    item_heights: Vec<usize>,
    area: Rect,
    total: usize,
}

/// All column rendering data computed during the immutable phase.
/// Passed to `render_columns` which performs the mutable rendering.
pub(super) struct ColumnsData {
    /// If in Epic view: `(title, outer_area)` for the epic border block.
    epic_border: Option<(String, Rect)>,
    /// Separator column areas (odd-indexed in the layout split).
    sep_areas: Vec<Rect>,
    /// Pre-built rendering data for each task-status column (one per TaskStatus::ALL entry).
    task_cols: Vec<TaskColData>,
    /// Pre-built rendering data for the archive column, if visible.
    archive_col: Option<ArchiveColData>,
}

// ---------------------------------------------------------------------------
// Immutable build helpers
// ---------------------------------------------------------------------------

/// Immutable inputs for [`build_task_col_data`], grouped so the builder takes a
/// single borrow-bundle rather than a wide positional parameter list. All fields
/// are immutable borrows, so the bundle can be constructed while a `ColumnLayout`
/// (which itself holds immutable borrows into `App`) is alive.
struct TaskColInput<'a> {
    app: &'a App,
    items: &'a [ColumnItem<'a>],
    epic_stats: &'a EpicStatsMap,
    col_area: Rect,
    now: DateTime<Utc>,
    status: TaskStatus,
    nav_col: usize,
}

/// Build the list items and selection state for a single task-status column.
///
/// Takes only immutable borrows so it can be called while a `ColumnLayout`
/// (which itself holds immutable borrows into `App`) is alive.
fn build_task_col_data(input: TaskColInput<'_>) -> TaskColData {
    let TaskColInput {
        app,
        items,
        epic_stats,
        col_area,
        now,
        status,
        nav_col,
    } = input;
    let col_idx = nav_col - 1;
    let is_focused = app.selected_column() == nav_col;
    let color = column_color(status);
    // In flat view the data layer pre-builds SubstatusLabel items; the renderer
    // must not also inject headers or they'd appear twice.
    let show_headers =
        !app.board.flattened && matches!(status, TaskStatus::Running | TaskStatus::Review);
    let selected_row = app.selected_row()[col_idx];
    let mut list_items: Vec<ListItem<'static>> = Vec::new();
    let mut item_heights: Vec<usize> = Vec::new();
    let mut list_selection_idx: Option<usize> = None;
    let mut current_priority: Option<u8> = None;

    // Helper: push an item and record its height in one step.
    macro_rules! push_item {
        ($item:expr) => {{
            let li: ListItem<'static> = $item;
            item_heights.push(li.height());
            list_items.push(li);
        }};
    }

    let mut selectable_idx: usize = 0;

    for item in items.iter() {
        // EpicHeader items are decorative — render immediately, don't affect
        // substatus grouping or cursor selection.
        if let ColumnItem::EpicHeader(epic) = item {
            push_item!(render_epic_header_item(
                epic,
                &app.board.epics,
                col_area.width
            ));
            continue;
        }

        if let ColumnItem::SubstatusLabel(label) = item {
            push_item!(render_substatus_header(label, list_items.is_empty()));
            continue;
        }

        if matches!(item, ColumnItem::OrphanSeparator) {
            push_item!(render_orphan_separator(
                col_area.width,
                list_items.is_empty()
            ));
            continue;
        }

        // Substatus grouping headers (Running / Review columns only).
        if show_headers {
            let priority = match item {
                ColumnItem::Task(t) => crate::models::task_column_priority(t),
                ColumnItem::Epic(e) => epic_stats
                    .get(&e.id)
                    .map(|s| s.substatus.column_priority())
                    .unwrap_or(0),
                ColumnItem::EpicHeader(_)
                | ColumnItem::SubstatusLabel(_)
                | ColumnItem::OrphanSeparator => unreachable!(),
            };
            if Some(priority) != current_priority {
                current_priority = Some(priority);
                let label = match item {
                    ColumnItem::Task(t) => crate::models::task_header_label(t).to_string(),
                    ColumnItem::Epic(e) => epic_stats
                        .get(&e.id)
                        .map(|s| s.substatus.header_label())
                        .unwrap_or_default()
                        .to_string(),
                    ColumnItem::EpicHeader(_)
                    | ColumnItem::SubstatusLabel(_)
                    | ColumnItem::OrphanSeparator => unreachable!(),
                };
                push_item!(render_substatus_header(&label, list_items.is_empty()));
            }
        }

        // Selection: cursor tracks selectable_idx, not the raw list position.
        if selectable_idx == selected_row {
            list_selection_idx = Some(list_items.len());
        }
        let is_cursor = is_focused && !app.on_select_all() && selectable_idx == selected_row;
        selectable_idx += 1;

        let ctx = ColRenderCtx {
            color,
            width: col_area.width,
            ground: column_bg_color(status, is_focused),
        };
        push_item!(match item {
            ColumnItem::Task(task) => {
                build_task_list_item(task, status, app, now, is_cursor, &ctx)
            }
            ColumnItem::Epic(epic) => {
                render_epic_item(epic, is_cursor, app, epic_stats, status, &ctx)
            }
            ColumnItem::EpicHeader(_)
            | ColumnItem::SubstatusLabel(_)
            | ColumnItem::OrphanSeparator => unreachable!(),
        });
    }

    TaskColData {
        list_items,
        list_selection_idx,
        item_heights,
        col_area,
        is_focused,
        status,
        color,
    }
}

/// Build list items for the archive column (immutable phase).
fn build_archive_col_data(
    app: &App,
    area: Rect,
    now: DateTime<Utc>,
    epic_stats: &EpicStatsMap,
) -> ArchiveColData {
    let archived_epics = app.archived_epics();
    let archived_tasks = app.archived_tasks();
    let sel_row = app.selected_archive_row();
    let color = column_color(TaskStatus::Archived);

    let mut items: Vec<ListItem<'static>> = Vec::new();
    let mut item_heights: Vec<usize> = Vec::new();

    // The archive column is only ever rendered while focused, so it takes the
    // focused ground — the same uniform neutral as every other column.
    let ctx = ColRenderCtx {
        color,
        width: area.width,
        ground: column_bg_color(TaskStatus::Archived, true),
    };

    for epic in archived_epics.iter() {
        let li = render_epic_item(epic, false, app, epic_stats, TaskStatus::Archived, &ctx);
        item_heights.push(li.height());
        items.push(li);
    }

    for (idx, task) in archived_tasks.iter().enumerate() {
        let is_cursor = idx == sel_row;
        let li = build_task_list_item(task, TaskStatus::Archived, app, now, is_cursor, &ctx);
        item_heights.push(li.height());
        items.push(li);
    }

    let total = archived_epics.len() + archived_tasks.len();

    ArchiveColData {
        items,
        item_heights,
        area,
        total,
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Compute all column rendering data using only immutable borrows of `app`.
///
/// This is the immutable phase of column rendering. It consumes the
/// pre-built `layout` (one sort per column, already done) so that the
/// mutable phase (`render_columns`) needs no further sorting.
///
/// Separated from `render_columns` so that `layout` can be dropped before
/// the mutable list-state updates that `render_columns` performs.
pub(super) fn compute_columns_data<'a>(
    app: &'a App,
    layout: &ColumnLayout<'a>,
    epic_stats: &EpicStatsMap,
    area: Rect,
    now: DateTime<Utc>,
) -> ColumnsData {
    // Determine the board area and, if in epic view, capture the title for the border.
    let (epic_border, board_area) = if let ViewMode::Epic {
        epic_id, parent, ..
    } = app.view_mode()
    {
        // Walk the parent chain to collect all ancestor IDs (innermost first),
        // then reverse for display order (root → … → current).
        let mut ids: Vec<EpicId> = vec![*epic_id];
        let mut cursor: &ViewMode = parent.as_ref();
        while let ViewMode::Epic {
            epic_id: pid,
            parent: grandparent,
            ..
        } = cursor
        {
            ids.push(*pid);
            cursor = grandparent.as_ref();
        }
        ids.reverse();
        let segments: Vec<String> = ids
            .iter()
            .map(|id| {
                app.epics()
                    .iter()
                    .find(|e| e.id == *id)
                    .map(|e| truncate(&e.title, 30))
                    .unwrap_or_default()
            })
            .collect();
        let title = format!(
            " {} ",
            segments.join(crate::tui::ui::shared::BREADCRUMB_SEPARATOR)
        );

        // Inner area: Borders::ALL removes 1 from each side.
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        (Some((title, area)), inner)
    } else {
        (None, area)
    };

    // Split board area into content columns and separators.
    let sel = app.selected_column();
    let all_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(board_column_constraints(sel))
        .split(board_area);

    let sep_areas: Vec<Rect> = (1..all_areas.len())
        .step_by(2)
        .map(|i| all_areas[i])
        .collect();
    let content_areas: Vec<Rect> = (0..all_areas.len())
        .step_by(2)
        .map(|i| all_areas[i])
        .collect();

    // Build task column data using the pre-computed layout.
    // Both `app` (&App reborrow) and `layout` (&ColumnLayout<'a> which holds &'a App)
    // are immutable borrows — Rust allows multiple simultaneous immutable borrows.
    let mut task_cols: Vec<TaskColData> = Vec::with_capacity(TaskStatus::COLUMN_COUNT);
    for (task_col_idx, &status) in TaskStatus::ALL.iter().enumerate() {
        let nav_col = task_col_idx + 1;
        let items = layout.get(status);
        let col_area = content_areas[task_col_idx];
        task_cols.push(build_task_col_data(TaskColInput {
            app,
            items,
            epic_stats,
            col_area,
            now,
            status,
            nav_col,
        }));
    }
    // `layout` is last used above; its immutable borrow on `app` ends here (NLL).

    // Archive column data, if the archive is the selected column.
    let archive_col = if sel == TaskStatus::COLUMN_COUNT + 1 {
        let archive_area = content_areas[TaskStatus::COLUMN_COUNT];
        Some(build_archive_col_data(app, archive_area, now, epic_stats))
    } else {
        None
    };

    ColumnsData {
        epic_border,
        sep_areas,
        task_cols,
        archive_col,
    }
}

/// Render all kanban columns using pre-computed `ColumnsData`.
///
/// This is the mutable phase: it updates `ListState` selection/scroll and
/// calls `render_stateful_widget`. The `ColumnsData` was produced by
/// `compute_columns_data`, which ran the immutable phase (item sorting and
/// list-item construction) while a `ColumnLayout` was still alive.
pub(super) fn render_columns(frame: &mut Frame, app: &mut App, data: ColumnsData) {
    // Render the epic view border if applicable.
    if let Some((title, outer_area)) = &data.epic_border {
        let block = rounded_block(PURPLE)
            .title(title.as_str())
            .title_style(Style::default().fg(PURPLE).add_modifier(Modifier::BOLD))
            .style(Style::default().bg(Color::Rgb(24, 20, 34)));
        frame.render_widget(block, *outer_area);
    }

    // Render column separators.
    for &sep_area in &data.sep_areas {
        render_column_separator(frame, sep_area);
    }

    // Task columns — mutable list-state update + render.
    {
        let on_select_all = app.on_select_all();
        let sel = app.selection_mut();
        for (col_idx, col_data) in data.task_cols.into_iter().enumerate() {
            if col_data.is_focused {
                *sel.list_states[col_idx].selected_mut() = if on_select_all {
                    None
                } else {
                    col_data.list_selection_idx
                };
            }

            let border_color = if col_data.is_focused {
                col_data.color
            } else {
                MUTED
            };
            // Every column paints the same uniform neutral ground; focus raises
            // its lightness rather than tinting it (core.allium: "Column ground
            // and card surface").
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(border_color))
                .style(Style::default().bg(column_bg_color(col_data.status, col_data.is_focused)));
            let inner = block.inner(col_data.col_area);
            frame.render_widget(block, col_data.col_area);
            frame.render_stateful_widget(
                List::new(col_data.list_items),
                inner,
                &mut sel.list_states[col_idx],
            );
            render_scroll_indicators(
                frame,
                &sel.list_states[col_idx],
                &col_data.item_heights,
                inner,
                col_data.col_area,
                border_color,
            );
        }
    } // `sel` is dropped here so `app` is available again for archive rendering.

    // Archive column.
    if let Some(archive_data) = data.archive_col {
        let bg_block = Block::default()
            .style(Style::default().bg(column_bg_color(TaskStatus::Archived, true)));
        frame.render_widget(bg_block, archive_data.area);

        let total = archive_data.total;
        let title = format!(" Archive ({total}) ");
        let block = Block::default()
            .title(title)
            .title_style(
                Style::default()
                    .fg(column_color(TaskStatus::Archived))
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::TOP)
            .border_style(Style::default().fg(column_color(TaskStatus::Archived)));
        let inner = block.inner(archive_data.area);
        let list = List::new(archive_data.items).block(block);
        frame.render_stateful_widget(list, archive_data.area, &mut app.archive.list_state);
        render_scroll_indicators(
            frame,
            &app.archive.list_state,
            &archive_data.item_heights,
            inner,
            archive_data.area,
            column_color(TaskStatus::Archived),
        );
    }
}

/// Draw ▲ / ▼ scroll indicators at the right edge of a column when content
/// overflows the visible area. Called after `render_stateful_widget` so the
/// `ListState` offset has already been updated by ratatui for this frame.
fn render_scroll_indicators(
    frame: &mut Frame,
    list_state: &ListState,
    item_heights: &[usize],
    inner: Rect,
    col_area: Rect,
    indicator_color: Color,
) {
    if col_area.width == 0 || col_area.height == 0 {
        return;
    }

    let offset = list_state.offset();
    let has_above = offset > 0;

    let visible_height = inner.height as usize;
    let remaining_height: usize = item_heights.get(offset..).unwrap_or_default().iter().sum();
    let has_below = remaining_height > visible_height;

    if !has_above && !has_below {
        return;
    }

    let style = Style::default()
        .fg(indicator_color)
        .add_modifier(Modifier::BOLD);
    let x = col_area.right().saturating_sub(1);
    let buf = frame.buffer_mut();

    if has_above {
        buf[(x, col_area.top())].set_symbol("▲").set_style(style);
    }
    if has_below {
        buf[(x, inner.bottom().saturating_sub(1))]
            .set_symbol("▼")
            .set_style(style);
    }
}
