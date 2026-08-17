use super::palette::{CYAN, MUTED, RED};
use crate::models::TaskId;
use crate::tui::App;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

/// Build the active input row for a single-line text field, drawing the caret
/// as a reversed block at `app.input.caret` and scrolling long values so the
/// caret stays visible. `prefix` includes the label and separator, e.g.
/// `"  Title: "`.
fn caret_field(prefix: &str, app: &App, area: Rect, active: Style) -> Line<'static> {
    super::caret_field_line(
        area.width,
        prefix,
        "",
        &app.input.buffer,
        app.input.caret,
        active,
    )
}

/// The sizing and styling context every repo-path picker list shares.
///
/// `height_offset` is the number of rows the surrounding form already spends
/// above and below the list; `area_height` is the popup's total height. The
/// list gets whatever is left. Bundled into one struct because these three
/// always travel together and are constant for a given picker surface.
pub(in crate::tui::ui) struct RepoListCtx {
    pub height_offset: u16,
    pub area_height: u16,
    pub hint: Style,
}

impl RepoListCtx {
    /// Rows available to the list itself, never less than one.
    fn visible_rows(&self) -> usize {
        super::shared::visible_rows(self.area_height as usize, self.height_offset as usize)
    }
}

/// Appends the filtered repo list and optional new-path entry to `lines`.
///
/// Shows existing paths that fuzzy-match `buffer`, then appends a selectable
/// new-path entry when `buffer` is non-empty and not an exact match for any
/// filtered item. This is the shared rendering contract for all
/// `RepoPathPicker` surfaces (InputRepoPath, MainSessionDir, QuickDispatch).
fn append_filtered_repos_with_new_entry<'a>(
    lines: &mut Vec<Line<'a>>,
    filtered: &[String],
    buffer: &'a str,
    cursor: usize,
    ctx: &RepoListCtx,
) {
    let show_new = crate::tui::has_new_repo_option(buffer, filtered);
    let scroll_cursor = if show_new && !filtered.is_empty() && cursor == filtered.len() {
        filtered.len() - 1
    } else {
        cursor
    };
    if !filtered.is_empty() {
        append_repo_path_list(lines, filtered, scroll_cursor, ctx);
    }
    let hint = ctx.hint;
    if show_new {
        let cursor_style = Style::default().fg(CYAN).add_modifier(Modifier::BOLD);
        if cursor == filtered.len() {
            lines.push(Line::from(vec![
                Span::styled("  ► ", cursor_style),
                Span::styled(buffer, cursor_style),
                Span::styled("  (new)", hint),
            ]));
        } else {
            lines.push(Line::from(Span::styled(format!("    + {buffer}"), hint)));
        }
    }
}

/// Appends a scrollable repo-path picker list to `lines`.
pub(in crate::tui::ui) fn append_repo_path_list<'a>(
    lines: &mut Vec<Line<'a>>,
    repo_paths: &[String],
    cursor: usize,
    ctx: &RepoListCtx,
) {
    let hint = ctx.hint;
    let visible_repos = ctx.visible_rows();
    let scroll = super::shared::scroll_offset(cursor, repo_paths.len(), visible_repos);
    let cursor_style = Style::default().fg(CYAN).add_modifier(Modifier::BOLD);
    for (i, path) in repo_paths
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_repos)
    {
        if i == cursor {
            lines.push(Line::from(vec![
                Span::styled("  ► ".to_string(), cursor_style),
                Span::styled(path.to_string(), cursor_style),
            ]));
        } else {
            lines.push(Line::from(Span::styled(format!("    {path}"), hint)));
        }
    }
}

pub(in crate::tui) fn input_title_lines(
    app: &App,
    area: Rect,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
    vec![
        caret_field("  Title: ", app, area, active),
        Line::from(""),
        Line::from(Span::styled("  [Enter] confirm  [Esc] cancel", hint)),
    ]
}

pub(in crate::tui) fn input_tag_lines(
    app: &App,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
    let title = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(
            "  Tag: [b]ug  [f]eature  [c]hore  [e]pic  [p]r-review  [r]esearch  [x]fix  [Enter] none",
            active,
        )),
        Line::from(""),
        Line::from(Span::styled("  [Enter] skip  [Esc] cancel", hint)),
    ]
}

pub(in crate::tui) fn input_description_lines(
    app: &App,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
    let title = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    let tag = app
        .input
        .task_draft
        .as_ref()
        .and_then(|d| d.tag.as_ref())
        .map(|t| t.to_string())
        .unwrap_or_else(|| "none".to_string());
    vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(format!("  Tag: {tag}"), completed)),
        Line::from(Span::styled(
            "  Description: opening $EDITOR...".to_string(),
            active,
        )),
        Line::from(""),
        Line::from(Span::styled("  [Esc] cancel", hint)),
    ]
}

pub(in crate::tui) fn input_repo_path_lines<'a>(
    app: &'a App,
    area: Rect,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'a>> {
    let title = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    let tag = app
        .input
        .task_draft
        .as_ref()
        .and_then(|d| d.tag.as_ref())
        .map(|t| t.to_string())
        .unwrap_or_else(|| "none".to_string());
    let description = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.description.as_str())
        .unwrap_or("");
    let desc_first_line = description.lines().next().unwrap_or("");
    let desc_display = if description.contains('\n') {
        format!("{desc_first_line} ...")
    } else {
        desc_first_line.to_string()
    };
    let mut lines = vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(format!("  Tag: {tag}"), completed)),
        Line::from(Span::styled(
            format!("  Description: {desc_display}"),
            completed,
        )),
        caret_field("  Repo path: ", app, area, active),
    ];
    let filtered = crate::tui::filtered_repos(&app.board.repo_paths, &app.input.buffer);
    append_filtered_repos_with_new_entry(
        &mut lines,
        &filtered,
        &app.input.buffer,
        app.input.repo_cursor,
        &RepoListCtx {
            height_offset: 7,
            area_height: area.height,
            hint,
        },
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type to filter · [↑/↓] navigate · [Enter] select · [Esc] cancel",
        hint,
    )));
    lines
}

pub(in crate::tui) fn input_base_branch_lines<'a>(
    app: &'a App,
    area: Rect,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'a>> {
    let title = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.title.clone())
        .unwrap_or_default();
    let tag = app
        .input
        .task_draft
        .as_ref()
        .and_then(|d| d.tag.as_ref())
        .map(|t| t.to_string())
        .unwrap_or_else(|| "none".to_string());
    let description = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.description.clone())
        .unwrap_or_default();
    let desc_first_line = description.lines().next().unwrap_or("").to_string();
    let desc_display = if description.contains('\n') {
        format!("{desc_first_line} ...")
    } else {
        desc_first_line
    };
    let repo_path = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.repo_path.clone())
        .unwrap_or_default();
    let mut lines = vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(format!("  Tag: {tag}"), completed)),
        Line::from(Span::styled(
            format!("  Description: {desc_display}"),
            completed,
        )),
        Line::from(Span::styled(format!("  Repo path: {repo_path}"), completed)),
        caret_field("  Base branch: ", app, area, active),
    ];
    let history = app.base_branches_for(&repo_path);
    let filtered = crate::tui::filtered_repos(history, &app.input.buffer);
    append_filtered_repos_with_new_entry(
        &mut lines,
        &filtered,
        &app.input.buffer,
        app.input.repo_cursor,
        &RepoListCtx {
            height_offset: 6,
            area_height: area.height,
            hint,
        },
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type to filter · [↑/↓] navigate · [Enter] select · [Esc] cancel",
        hint,
    )));
    lines
}

/// The steps answered before the wrap-up picker, restated above whichever step
/// is active. Shared by the whole tail of the creation form so the four
/// summary lines are written once rather than once per step.
fn answered_step_lines(app: &App, completed: Style) -> Vec<Line<'static>> {
    let draft = app.input.task_draft.as_ref();
    let title = draft.map(|d| d.title.clone()).unwrap_or_default();
    let tag = draft
        .and_then(|d| d.tag.as_ref())
        .map(|t| t.to_string())
        .unwrap_or_else(|| "none".to_string());
    let repo_path = draft.map(|d| d.repo_path.clone()).unwrap_or_default();
    let base_branch = draft
        .map(|d| d.base_branch.clone())
        .unwrap_or_else(|| "main".to_string());
    vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(format!("  Tag: {tag}"), completed)),
        Line::from(Span::styled(format!("  Repo: {repo_path}"), completed)),
        Line::from(Span::styled(
            format!("  Base branch: {base_branch}"),
            completed,
        )),
    ]
}

/// The wrap-up answer as a settled summary line, for the steps after it.
fn answered_wrap_up_line(app: &App, completed: Style) -> Line<'static> {
    let wrap_up = app
        .input
        .task_draft
        .as_ref()
        .and_then(|d| d.wrap_up_mode)
        .map(|m| m.as_str())
        .unwrap_or("none");
    Line::from(Span::styled(format!("  Wrap-up: {wrap_up}"), completed))
}

/// Close off a creation-form step page: the active line, a blank, the Esc hint.
///
/// Every tail step has the same shape — settled summaries, one active line,
/// then this footer — so the footer is written once. `active` is the one part
/// that genuinely differs: a styled span for the single-key pickers, a
/// [`caret_field`] for the free-text steps.
fn form_step_page<'a>(mut settled: Vec<Line<'a>>, active: Line<'a>, hint: Style) -> Vec<Line<'a>> {
    settled.push(active);
    settled.push(Line::from(""));
    settled.push(Line::from(Span::styled("  [Esc] cancel", hint)));
    settled
}

/// The two-space indent every prompt is rendered with inside the form panel.
/// The prompt constants themselves are unindented because the status bar shows
/// them flush.
fn indented(prompt: &str) -> String {
    format!("  {prompt}")
}

pub(in crate::tui) fn input_wrap_up_mode_lines(
    app: &App,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
    form_step_page(
        answered_step_lines(app, completed),
        Line::from(Span::styled(
            "  Wrap-up: [r]ebase  [p]r  [d]one  [Enter] skip",
            active,
        )),
        hint,
    )
}

/// The gate: one keypress deciding whether the two scheduling fields are
/// configured at all. Enter is the common answer, so it is listed first.
pub(in crate::tui) fn input_schedule_gate_lines(
    app: &App,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
    let mut settled = answered_step_lines(app, completed);
    settled.push(answered_wrap_up_line(app, completed));
    form_step_page(
        settled,
        Line::from(Span::styled(
            indented(crate::tui::update::SCHEDULE_GATE_PROMPT),
            active,
        )),
        hint,
    )
}

pub(in crate::tui) fn input_schedule_interval_lines<'a>(
    app: &'a App,
    area: Rect,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'a>> {
    let mut settled = answered_step_lines(app, completed);
    settled.push(answered_wrap_up_line(app, completed));
    let prompt = indented(&crate::tui::update::schedule_interval_prompt());
    form_step_page(settled, caret_field(&prompt, app, area, active), hint)
}

pub(in crate::tui) fn input_pinned_branch_lines<'a>(
    app: &'a App,
    area: Rect,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'a>> {
    let mut settled = answered_step_lines(app, completed);
    settled.push(answered_wrap_up_line(app, completed));
    let interval = app
        .input
        .task_draft
        .as_ref()
        .and_then(|d| d.schedule_interval_secs)
        .map(crate::models::format_interval_secs)
        .unwrap_or_else(|| "none".to_string());
    settled.push(Line::from(Span::styled(
        format!("  Schedule: {interval}"),
        completed,
    )));
    let prompt = indented(crate::tui::update::PINNED_BRANCH_PROMPT);
    form_step_page(settled, caret_field(&prompt, app, area, active), hint)
}

fn repo_picker_lines<'a>(
    app: &'a App,
    area: Rect,
    header: &'a str,
    prefix: &'a str,
    hint_text: &'a str,
    active: Style,
    hint: Style,
) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(Span::styled(header, active)),
        Line::from(""),
        caret_field(&format!("  {prefix}: "), app, area, active),
    ];
    let filtered = crate::tui::filtered_repos(&app.board.repo_paths, &app.input.buffer);
    append_filtered_repos_with_new_entry(
        &mut lines,
        &filtered,
        &app.input.buffer,
        app.input.repo_cursor,
        &RepoListCtx {
            height_offset: 7,
            area_height: area.height,
            hint,
        },
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(hint_text, hint)));
    lines
}

pub(in crate::tui) fn main_session_dir_lines<'a>(
    app: &'a App,
    area: Rect,
    active: Style,
    hint: Style,
) -> Vec<Line<'a>> {
    repo_picker_lines(
        app,
        area,
        "  Main session — base repo:",
        "Path",
        "  Type to filter · [↑/↓] navigate · [Enter] select · [Esc] cancel",
        active,
        hint,
    )
}

pub(in crate::tui) fn quick_dispatch_lines<'a>(
    app: &'a App,
    area: Rect,
    active: Style,
    hint: Style,
) -> Vec<Line<'a>> {
    let filtered = crate::tui::filtered_repos(&app.board.repo_paths, &app.input.buffer);
    let mut lines = vec![
        Line::from(Span::styled("  Quick Dispatch — select repo:", active)),
        Line::from(""),
        caret_field("  Filter: ", app, area, active),
    ];
    append_filtered_repos_with_new_entry(
        &mut lines,
        &filtered,
        &app.input.buffer,
        app.input.repo_cursor,
        &RepoListCtx {
            height_offset: 7,
            area_height: area.height,
            hint,
        },
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type to filter · [↑/↓] navigate · [Enter] select · [Esc] cancel",
        hint,
    )));
    lines
}

pub(in crate::tui) fn confirm_retry_lines(app: &App, id: TaskId) -> Vec<Line<'static>> {
    let label = if app.is_crashed(id) {
        "crashed"
    } else {
        "stale"
    };
    let warning = Style::default().fg(RED).add_modifier(Modifier::BOLD);
    let hint = Style::default().fg(MUTED);
    vec![
        Line::from(Span::styled(
            format!("  Agent is {label}. What do you want to do?"),
            warning,
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  [r] Resume (--continue in existing worktree)",
            hint,
        )),
        Line::from(Span::styled(
            "  [f] Fresh start (clean worktree + new dispatch)",
            hint,
        )),
        Line::from(Span::styled("  [Esc] Cancel", hint)),
    ]
}

pub(in crate::tui) fn input_epic_title_lines(
    app: &App,
    area: Rect,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
    vec![
        caret_field("  Title: ", app, area, active),
        Line::from(""),
        Line::from(Span::styled("  [Enter] confirm  [Esc] cancel", hint)),
    ]
}

pub(in crate::tui) fn input_epic_description_lines(
    app: &App,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
    let title = app
        .input
        .epic_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    vec![
        Line::from(Span::styled(format!("  Title: {title}"), completed)),
        Line::from(Span::styled(
            "  Description: opening $EDITOR...".to_string(),
            active,
        )),
        Line::from(""),
        Line::from(Span::styled("  [Esc] cancel", hint)),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Concatenate a line's span contents into a single string for assertions.
    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn owned(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    fn ctx(height_offset: u16, area_height: u16, hint: Style) -> RepoListCtx {
        RepoListCtx {
            height_offset,
            area_height,
            hint,
        }
    }

    // ---- append_repo_path_list -------------------------------------------

    #[test]
    fn repo_list_no_scroll_when_all_fit() {
        let hint = Style::default();
        let paths = owned(&["a", "b", "c"]);
        let mut lines: Vec<Line> = Vec::new();
        // visible = area_height - height_offset = 10; 3 <= 10 → scroll = 0.
        append_repo_path_list(&mut lines, &paths, 1, &ctx(0, 10, hint));

        assert_eq!(lines.len(), 3);
        assert_eq!(line_text(&lines[0]), "    a");
        // Cursor row is rendered as "  ► " + path across two spans.
        assert_eq!(line_text(&lines[1]), "  ► b");
        assert_eq!(lines[1].spans.len(), 2);
        assert_eq!(line_text(&lines[2]), "    c");
    }

    #[test]
    fn repo_list_scrolls_to_keep_cursor_visible() {
        let hint = Style::default();
        let paths = owned(&["p0", "p1", "p2", "p3", "p4"]);
        let mut lines: Vec<Line> = Vec::new();
        // visible = 3, cursor at last item → scroll = 4.sat_sub(2)=2, min(5-3=2)=2.
        append_repo_path_list(&mut lines, &paths, 4, &ctx(0, 3, hint));

        assert_eq!(lines.len(), 3);
        assert_eq!(line_text(&lines[0]), "    p2");
        assert_eq!(line_text(&lines[1]), "    p3");
        assert_eq!(line_text(&lines[2]), "  ► p4");
    }

    #[test]
    fn repo_list_visible_height_floors_at_one() {
        let hint = Style::default();
        let paths = owned(&["p0", "p1", "p2"]);
        let mut lines: Vec<Line> = Vec::new();
        // height_offset >= area_height → saturating_sub is 0, floored to 1 visible row.
        append_repo_path_list(&mut lines, &paths, 2, &ctx(5, 3, hint));

        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "  ► p2");
    }

    // ---- append_filtered_repos_with_new_entry ----------------------------

    #[test]
    fn filtered_new_entry_highlighted_when_cursor_past_list() {
        let hint = Style::default();
        let filtered = owned(&["a", "b"]);
        let mut lines: Vec<Line> = Vec::new();
        // buffer "c" is new; cursor == filtered.len() selects the new entry.
        append_filtered_repos_with_new_entry(&mut lines, &filtered, "c", 2, &ctx(1, 40, hint));

        // Two existing paths + the highlighted new entry.
        assert_eq!(lines.len(), 3);
        let last = &lines[2];
        assert_eq!(last.spans.len(), 3);
        assert_eq!(line_text(last), "  ► c  (new)");
        // With the cursor on the new entry, the list itself must not also
        // highlight a row (scroll_cursor clamps back to filtered.len()-1 = 1).
        assert_eq!(line_text(&lines[1]), "  ► b");
    }

    #[test]
    fn filtered_new_entry_dimmed_when_cursor_in_list() {
        let hint = Style::default();
        let filtered = owned(&["a", "b"]);
        let mut lines: Vec<Line> = Vec::new();
        // buffer "c" is new but cursor points at an existing row.
        append_filtered_repos_with_new_entry(&mut lines, &filtered, "c", 0, &ctx(1, 40, hint));

        assert_eq!(lines.len(), 3);
        assert_eq!(line_text(&lines[0]), "  ► a");
        assert_eq!(line_text(&lines[2]), "    + c");
    }

    #[test]
    fn filtered_no_new_entry_for_empty_buffer() {
        let hint = Style::default();
        let filtered = owned(&["a", "b"]);
        let mut lines: Vec<Line> = Vec::new();
        // Empty buffer → no "new path" entry offered.
        append_filtered_repos_with_new_entry(&mut lines, &filtered, "", 0, &ctx(1, 40, hint));

        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn filtered_no_new_entry_when_buffer_matches_existing() {
        let hint = Style::default();
        let filtered = owned(&["a", "b"]);
        let mut lines: Vec<Line> = Vec::new();
        // buffer exactly equals a filtered path → not a "new" entry.
        append_filtered_repos_with_new_entry(&mut lines, &filtered, "b", 1, &ctx(1, 40, hint));

        assert_eq!(lines.len(), 2);
    }
}
