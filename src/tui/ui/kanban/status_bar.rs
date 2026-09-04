//! Status bar at the bottom of the kanban board.
//!
//! Renders one of three flavours depending on app state:
//! * a transient status message,
//! * archive-mode hints, or
//! * mode-specific hints (Normal mode delegates to `action_hints` /
//!   `epic_action_hints` / `batch_action_hints`).

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::super::palette::{CYAN, GREEN, MUTED, PURPLE, RED, YELLOW};
use super::super::shared::push_hint_spans;
use super::{action_hints, epic_action_hints};
use crate::tui::{App, ColumnItem, InputMode};

pub(super) fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let (line, style) = status_line(app, area);
    frame.render_widget(Paragraph::new(line).style(style), area);
}

/// Prepend `prefix` spans to `spans` in place, preserving order.
fn prepend(spans: &mut Vec<Span<'static>>, mut prefix: Vec<Span<'static>>) {
    prefix.append(spans);
    *spans = prefix;
}

/// A simple text status line with a single foreground colour, falling back to
/// `app.status.message` when set. (`app.status.message` being `Some` is already
/// short-circuited by `status_line`, so this always resolves to `default`; the
/// override arms retain this shape for clarity.)
fn hint_text(app: &App, default: &str, color: Color) -> (Line<'static>, Style) {
    let text = app.status.message.as_deref().unwrap_or(default).to_string();
    (Line::from(text), Style::default().fg(color))
}

/// A fixed text status line with a single foreground colour. Accepts anything
/// that converts into a `Line<'static>`, so string literals borrow without an
/// allocation while owned `String`s (e.g. a formatted search prompt) move in.
fn hint(text: impl Into<Line<'static>>, color: Color) -> (Line<'static>, Style) {
    (text.into(), Style::default().fg(color))
}

/// Build the repo-drift segment for the status bar, or `None` when it should be
/// hidden (docs/specs/repo-sync.allium: surface RepoDriftIndicator).
///
/// Rendered only for a *measured* repository with real drift: no selected task,
/// an unmeasurable repository and a clean one all yield nothing, so the segment
/// can never claim "in sync" about a repository it could not measure
/// (`UnmeasuredIsNeverPresentedAsClean`). Any `behind > 0` is styled as a
/// warning — that is the direction that will bite the next rebase — while
/// ahead-only is neutral, being the normal state after every rebase wrap-up.
pub(in crate::tui) fn repo_drift_segment(
    state: Option<&crate::repo_sync::RepoSyncState>,
) -> Option<Vec<Span<'static>>> {
    let state = state?;
    let counts = state.counts?;
    if !counts.has_drift() {
        return None;
    }
    let color = if counts.behind > 0 { YELLOW } else { MUTED };
    Some(vec![Span::styled(
        format!(
            "{} \u{2191}{}\u{2193}{} ",
            state.base_branch, counts.ahead, counts.behind
        ),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )])
}

/// Display width allotted to the repository path inside the sync prompt
/// (docs/specs/repo-sync.allium: `path_display_budget` on surface
/// RepoSyncConfirmation).
pub(in crate::tui) const REPO_PATH_DISPLAY_BUDGET: usize = 40;

/// Render `repo_path` for the sync prompt, shortened from the *left* when it
/// exceeds `REPO_PATH_DISPLAY_BUDGET` (`PromptNamesTheRepository`).
///
/// The status bar has finite width, and a right-truncated path loses exactly the
/// part that tells two checkouts apart — so the head is elided, marked with an
/// ellipsis, and the distinguishing tail is kept. The cut lands on a path
/// separator so the result still reads as a path; only a final component that is
/// itself over budget is cut mid-component.
pub(in crate::tui) fn repo_path_for_prompt(repo_path: &str) -> String {
    if repo_path.chars().count() <= REPO_PATH_DISPLAY_BUDGET {
        return repo_path.to_string();
    }
    // One char of the budget pays for the ellipsis marking the elided head.
    let tail_budget = REPO_PATH_DISPLAY_BUDGET.saturating_sub(1);
    // Separator positions run head-to-tail, so the first suffix that fits is the
    // longest run of whole components that fits.
    let on_separator = repo_path
        .char_indices()
        .filter(|&(_, c)| c == '/')
        .map(|(byte_idx, _)| &repo_path[byte_idx..])
        .find(|tail| tail.chars().count() <= tail_budget);
    let tail = match on_separator {
        Some(tail) => tail,
        None => {
            // Not even the final component fits: keep the last chars of it.
            let skip = repo_path.chars().count().saturating_sub(tail_budget);
            let byte_idx = repo_path
                .char_indices()
                .nth(skip)
                .map_or(repo_path.len(), |(byte_idx, _)| byte_idx);
            &repo_path[byte_idx..]
        }
    };
    format!("…{tail}")
}

/// The sync confirmation prompt for one repository
/// (docs/specs/repo-sync.allium: surface RepoSyncConfirmation).
///
/// Names the operations that will actually run against origin, with their commit
/// counts, and no others: a half that will not run is not mentioned
/// (`PromptStatesExactlyWhatWillHappen`). It also names the repository, not only
/// the branch, so two repositories sitting on the same branch never produce the
/// same prompt immediately before the only network write dispatch performs to a
/// shared branch (`PromptNamesTheRepository`).
pub(in crate::tui) fn repo_sync_prompt_text(state: &crate::repo_sync::RepoSyncState) -> String {
    let (ahead, behind) = state.counts.map_or((0, 0), |c| (c.ahead, c.behind));
    let mut halves: Vec<String> = Vec::new();
    if behind > 0 {
        halves.push(format!("merge {behind} from origin"));
    }
    if ahead > 0 {
        halves.push(format!("push {ahead} to origin"));
    }
    format!(
        "Sync {} in {}: {}? [y/n]",
        state.base_branch,
        repo_path_for_prompt(&state.repo_path),
        halves.join(", ")
    )
}

/// Compute the status bar content (a styled `Line` plus a base paragraph style)
/// for the current app state. Rendering happens once, in `render_status_bar`.
///
/// The two structurally-heavier flavours — archive-mode hints and the composed
/// Normal-mode hint line — live in dedicated builders (`archive_status_line`,
/// `normal_status_line`); everything else is a fixed per-mode hint.
fn status_line(app: &App, area: Rect) -> (Line<'static>, Style) {
    if let Some(msg) = &app.status.message {
        return (Line::from(msg.clone()), Style::default().fg(YELLOW));
    }

    // Archive mode status bar
    if app.show_archived() {
        return archive_status_line();
    }

    match &app.input.mode {
        InputMode::Normal => normal_status_line(app),
        InputMode::SearchTasks => hint(
            format!(
                "Search board: {}_   [Enter] keep  [Esc] cancel",
                app.search.query
            ),
            CYAN,
        ),
        InputMode::InputTitle => hint("Creating task: enter title", YELLOW),
        InputMode::InputDescription => {
            hint("Creating task: opening $EDITOR for description", YELLOW)
        }
        InputMode::InputRepoPath => hint("Creating task: enter repo path", YELLOW),
        InputMode::InputTag => hint_text(
            app,
            crate::tui::ui::tag_prompt(app.input.phoenix_armed()),
            YELLOW,
        ),
        InputMode::ConfirmDelete => hint_text(app, "Delete? [y/n]", RED),
        InputMode::QuickDispatch => hint("Quick dispatch: select repo path", YELLOW),
        InputMode::ConfirmRetry(_) => hint("[r] Resume  [f] Fresh start  [Esc] Cancel", RED),
        InputMode::ConfirmArchive(_) => hint("Archive task? [y/n]", YELLOW),
        InputMode::ConfirmDone => hint_text(app, "Move to Done? [y/n]", YELLOW),
        InputMode::InputEpicTitle => hint("Creating epic: enter title", PURPLE),
        InputMode::InputEpicDescription => {
            hint("Creating epic: opening $EDITOR for description", PURPLE)
        }
        InputMode::ConfirmDeleteEpic => hint_text(app, "Delete epic and subtasks? [y/n]", RED),
        InputMode::ConfirmArchiveEpic => hint("Archive epic and subtasks? [y/n]", YELLOW),
        InputMode::Help => hint("[?] or [Esc] to close help", CYAN),
        InputMode::RepoFilter => hint("Filter repos: [1-9] toggle  [a] all  [q/Esc] close", CYAN),
        InputMode::InputPresetName => hint("Enter preset name, [Enter] save, [Esc] cancel", CYAN),
        InputMode::ConfirmDeletePreset => hint("[A-Z] delete preset  [Esc] cancel", CYAN),
        InputMode::ConfirmDeleteRepoPath => {
            hint("Delete repo path? y to confirm, any key to cancel", YELLOW)
        }
        InputMode::ConfirmDetachTmux(_) => hint_text(app, "Detach tmux panel? [y/n]", YELLOW),
        InputMode::ConfirmQuit => hint("Quit dispatch? [y/n]", YELLOW),
        InputMode::InputBaseBranch => hint_text(app, "Base branch: ", YELLOW),
        InputMode::InputWrapUpMode => {
            hint_text(app, "Wrap-up: [r]ebase  [p]r  [d]one  [Enter] skip", YELLOW)
        }
        InputMode::ReparentEpic(_) => hint(
            "Select new parent: navigate tree above, Enter to select",
            PURPLE,
        ),
        InputMode::ConfirmReparentEpic { .. } => hint_text(app, "Reparent epic? [y/n]", PURPLE),
        InputMode::MoveTaskToEpic(_) => hint(
            "Select target epic: navigate tree above, Enter to select",
            PURPLE,
        ),
        InputMode::ConfirmMoveTaskToEpic { .. } => {
            hint_text(app, "Move task to epic? [y/n]", PURPLE)
        }
        InputMode::TodoTitle | InputMode::TodoQuickAdd => {
            let label = if matches!(app.input.mode, InputMode::TodoTitle) {
                "New todo"
            } else {
                "Quick add"
            };
            let line = crate::tui::ui::caret_field_line(
                area.width,
                &format!("{label}: "),
                "  [Enter] save  [Esc] cancel",
                &app.input.buffer,
                app.input.caret,
                Style::default().fg(YELLOW),
            );
            (line, Style::default())
        }
        InputMode::ConfirmDeleteTodo => hint("Delete todo? [y/n]", RED),
        InputMode::LinkTodoToTask(_) => hint_text(
            app,
            "Navigate to a task or epic and press Enter to link — Esc to cancel",
            CYAN,
        ),
        InputMode::ConfirmTrustRepo { .. } | InputMode::ConfirmTrustRepoQuickDispatch { .. } => {
            hint_text(app, "Repo not trusted — trust it? [y/N]", YELLOW)
        }
        InputMode::ConfirmRepoSync { repo_path } => {
            // The status message carrying the full prompt auto-clears after
            // STATUS_MESSAGE_TTL while the mode lives on, so the fallback rebuilds
            // the same prompt rather than degrading to one that names no
            // repository (`PromptNamesTheRepository`). A repository that lost its
            // drift — or its measurement — in the meantime is offered no sync at
            // all: confirming would be refused anyway, so the fallback must not
            // word itself as a sync that will happen
            // (`PromptStatesExactlyWhatWillHappen`).
            let fallback = app
                .repo_sync
                .get(repo_path)
                .filter(|state| state.has_drift())
                .map_or_else(
                    || "Nothing left to sync — [n] to dismiss".to_string(),
                    repo_sync_prompt_text,
                );
            hint_text(app, &fallback, YELLOW)
        }
    }
}

/// Archive-mode status bar: a fixed row of `[key] label` hints.
fn archive_status_line() -> (Line<'static>, Style) {
    let key_color = MUTED;
    let label_style = Style::default().fg(MUTED);
    let key_style = Style::default().fg(key_color).add_modifier(Modifier::BOLD);
    let spans = vec![
        Span::styled("[x]", key_style),
        Span::styled(" delete  ", label_style),
        Span::styled("[e]", key_style),
        Span::styled(" edit  ", label_style),
        Span::styled("[H]", key_style),
        Span::styled(" close  ", label_style),
        Span::styled("[q]", key_style),
        Span::styled(" quit  ", label_style),
    ];
    (Line::from(spans), Style::default())
}

/// Normal-mode status bar: the base action hints (batch / epic / task) with the
/// active-mode badges (split, flat, active-filter, search) and the
/// open-todo count composed around them.
fn normal_status_line(app: &App) -> (Line<'static>, Style) {
    let key_color = CYAN;
    let mut spans = if app.has_selection() {
        let count = app.selected_tasks().len() + app.selected_epics().len();
        let has_tasks = !app.selected_tasks().is_empty();
        batch_action_hints(count, key_color, has_tasks)
    } else if let Some(ColumnItem::Epic(epic)) = app.selected_column_item() {
        epic_action_hints(epic, key_color)
    } else {
        let task = app.selected_task();
        let now = chrono::Utc::now();
        let in_flight = task.is_some_and(|t| app.dispatch_may_be_in_flight(t, now));
        action_hints(task, in_flight, key_color)
    };
    if app.split_active() {
        prepend(
            &mut spans,
            vec![
                Span::styled(
                    "[s]",
                    Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
                ),
                Span::styled("plit ", Style::default().fg(GREEN)),
            ],
        );
    }
    if app.board.flattened {
        prepend(
            &mut spans,
            vec![Span::styled(
                "[flat] ",
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            )],
        );
    }
    if app.filter_only_active() {
        prepend(
            &mut spans,
            vec![Span::styled(
                "[active] ",
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            )],
        );
    }
    if app.search_active() {
        prepend(
            &mut spans,
            vec![Span::styled(
                format!("[/{}] ", app.search.query),
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            )],
        );
    }
    if let Some(segment) = repo_drift_segment(app.selected_repo_sync_state()) {
        prepend(&mut spans, segment);
    }
    if app.board.todo_open_count > 0 {
        spans.push(Span::styled(
            format!(" ({}) ", app.board.todo_open_count),
            Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
        ));
    }
    (Line::from(spans), Style::default())
}

/// Build status bar hints when tasks are batch-selected.
fn batch_action_hints(count: usize, key_color: Color, has_tasks: bool) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(MUTED);
    let count_style = Style::default().fg(YELLOW).add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(format!("{count} selected  "), count_style));

    let mut push_hint = |key: &'static str, label: &'static str| {
        push_hint_spans(&mut spans, key, label, key_color, label_style);
    };

    if has_tasks {
        push_hint("L", "move");
        push_hint("H", "back");
    }
    // 'x' completes tasks that aren't Done yet and archives the rest, so with
    // tasks selected the label can't commit to one verb. An epics-only
    // selection always archives.
    push_hint("x", if has_tasks { "done/archive" } else { "archive" });
    push_hint("a", "select all");
    push_hint("F", "flat");
    push_hint("v", "toggle");
    push_hint("Esc", "clear");
    spans
}
