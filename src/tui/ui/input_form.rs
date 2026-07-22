use crate::models::TaskId;
use crate::tui::App;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
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
    height_offset: u16,
    area_height: u16,
    hint: Style,
) {
    let show_new = crate::tui::has_new_repo_option(buffer, filtered);
    let scroll_cursor = if show_new && !filtered.is_empty() && cursor == filtered.len() {
        filtered.len() - 1
    } else {
        cursor
    };
    if !filtered.is_empty() {
        append_repo_path_list(
            lines,
            filtered,
            scroll_cursor,
            height_offset,
            area_height,
            hint,
        );
    }
    if show_new {
        let cursor_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
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
pub(in crate::tui) fn append_repo_path_list<'a>(
    lines: &mut Vec<Line<'a>>,
    repo_paths: &[String],
    cursor: usize,
    height_offset: u16,
    area_height: u16,
    hint: Style,
) {
    let repo_count = repo_paths.len();
    let visible_repos = (area_height.saturating_sub(height_offset) as usize).max(1);
    let scroll = if repo_count <= visible_repos {
        0
    } else {
        cursor
            .saturating_sub(visible_repos - 1)
            .min(repo_count - visible_repos)
    };
    let cursor_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
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
        7,
        area.height,
        hint,
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
        6,
        area.height,
        hint,
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type to filter · [↑/↓] navigate · [Enter] select · [Esc] cancel",
        hint,
    )));
    lines
}

pub(in crate::tui) fn input_wrap_up_mode_lines(
    app: &App,
    completed: Style,
    active: Style,
    hint: Style,
) -> Vec<Line<'static>> {
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
        Line::from(Span::styled(
            "  Wrap-up: [r]ebase  [p]r  [d]one  [Enter] skip",
            active,
        )),
        Line::from(""),
        Line::from(Span::styled("  [Esc] cancel", hint)),
    ]
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
        7,
        area.height,
        hint,
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
        7,
        area.height,
        hint,
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
    let warning = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    let hint = Style::default().fg(Color::DarkGray);
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

    // ---- append_repo_path_list -------------------------------------------

    #[test]
    fn repo_list_no_scroll_when_all_fit() {
        let hint = Style::default();
        let paths = owned(&["a", "b", "c"]);
        let mut lines: Vec<Line> = Vec::new();
        // visible = area_height - height_offset = 10; 3 <= 10 → scroll = 0.
        append_repo_path_list(&mut lines, &paths, 1, 0, 10, hint);

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
        append_repo_path_list(&mut lines, &paths, 4, 0, 3, hint);

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
        append_repo_path_list(&mut lines, &paths, 2, 5, 3, hint);

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
        append_filtered_repos_with_new_entry(&mut lines, &filtered, "c", 2, 1, 40, hint);

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
        append_filtered_repos_with_new_entry(&mut lines, &filtered, "c", 0, 1, 40, hint);

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
        append_filtered_repos_with_new_entry(&mut lines, &filtered, "", 0, 1, 40, hint);

        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn filtered_no_new_entry_when_buffer_matches_existing() {
        let hint = Style::default();
        let filtered = owned(&["a", "b"]);
        let mut lines: Vec<Line> = Vec::new();
        // buffer exactly equals a filtered path → not a "new" entry.
        append_filtered_repos_with_new_entry(&mut lines, &filtered, "b", 1, 1, 40, hint);

        assert_eq!(lines.len(), 2);
    }
}
