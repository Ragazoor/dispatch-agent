use super::palette::{CYAN, MUTED, RED};
use crate::models::TaskId;
use crate::tui::{App, InputState};
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

/// The already-answered form fields, rendered once for every step that shows
/// them back to the user. Owns the presentation fallbacks (`""` for a missing
/// title, `"none"` for a missing tag) so they cannot diverge per step.
struct DraftSummary {
    title: String,
    tag: String,
    /// The description's first line, suffixed `" ..."` when more lines follow.
    description_oneline: String,
}

impl DraftSummary {
    fn from_input(input: &InputState) -> Self {
        let draft = input.task_draft.as_ref();
        let title = draft.map(|d| d.title.clone()).unwrap_or_default();
        let tag = draft
            .and_then(|d| d.tag.as_ref())
            .map(|t| t.to_string())
            .unwrap_or_else(|| "none".to_string());
        let description = draft.map(|d| d.description.as_str()).unwrap_or("");
        let desc_first_line = description.lines().next().unwrap_or("").to_string();
        let description_oneline = if description.contains('\n') {
            format!("{desc_first_line} ...")
        } else {
            desc_first_line
        };
        Self {
            title,
            tag,
            description_oneline,
        }
    }
}

/// The three styles a form step draws with: settled fields above the cursor,
/// the active field, and trailing hint text. Bundled so the 11 step-renderer
/// functions below take one reference instead of transposable positional
/// `Style` params.
pub(in crate::tui::ui) struct FormStyles {
    pub completed: Style,
    pub active: Style,
    pub hint: Style,
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
    styles: &FormStyles,
) -> Vec<Line<'static>> {
    vec![
        caret_field("  Title: ", app, area, styles.active),
        Line::from(""),
        Line::from(Span::styled("  [Enter] confirm  [Esc] cancel", styles.hint)),
    ]
}

pub(in crate::tui) fn input_tag_lines(app: &App, styles: &FormStyles) -> Vec<Line<'static>> {
    let summary = DraftSummary::from_input(&app.input);
    vec![
        Line::from(Span::styled(
            format!("  Title: {}", summary.title),
            styles.completed,
        )),
        Line::from(Span::styled(
            "  Tag: [b]ug  [f]eature  [c]hore  [p]r-review  [r]esearch  [x]fix  [Enter] none",
            styles.active,
        )),
        Line::from(""),
        Line::from(Span::styled("  [Enter] skip  [Esc] cancel", styles.hint)),
    ]
}

pub(in crate::tui) fn input_description_lines(
    app: &App,
    styles: &FormStyles,
) -> Vec<Line<'static>> {
    let summary = DraftSummary::from_input(&app.input);
    vec![
        Line::from(Span::styled(
            format!("  Title: {}", summary.title),
            styles.completed,
        )),
        Line::from(Span::styled(
            format!("  Tag: {}", summary.tag),
            styles.completed,
        )),
        Line::from(Span::styled(
            "  Description: opening $EDITOR...".to_string(),
            styles.active,
        )),
        Line::from(""),
        Line::from(Span::styled("  [Esc] cancel", styles.hint)),
    ]
}

pub(in crate::tui) fn input_repo_path_lines<'a>(
    app: &'a App,
    area: Rect,
    styles: &FormStyles,
) -> Vec<Line<'a>> {
    let summary = DraftSummary::from_input(&app.input);
    let mut lines = vec![
        Line::from(Span::styled(
            format!("  Title: {}", summary.title),
            styles.completed,
        )),
        Line::from(Span::styled(
            format!("  Tag: {}", summary.tag),
            styles.completed,
        )),
        Line::from(Span::styled(
            format!("  Description: {}", summary.description_oneline),
            styles.completed,
        )),
        caret_field("  Repo path: ", app, area, styles.active),
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
            hint: styles.hint,
        },
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type to filter · [↑/↓] navigate · [Enter] select · [Esc] cancel",
        styles.hint,
    )));
    lines
}

pub(in crate::tui) fn input_base_branch_lines<'a>(
    app: &'a App,
    area: Rect,
    styles: &FormStyles,
) -> Vec<Line<'a>> {
    let summary = DraftSummary::from_input(&app.input);
    let repo_path = app
        .input
        .task_draft
        .as_ref()
        .map(|d| d.repo_path.clone())
        .unwrap_or_default();
    let mut lines = vec![
        Line::from(Span::styled(
            format!("  Title: {}", summary.title),
            styles.completed,
        )),
        Line::from(Span::styled(
            format!("  Tag: {}", summary.tag),
            styles.completed,
        )),
        Line::from(Span::styled(
            format!("  Description: {}", summary.description_oneline),
            styles.completed,
        )),
        Line::from(Span::styled(
            format!("  Repo path: {repo_path}"),
            styles.completed,
        )),
        caret_field("  Base branch: ", app, area, styles.active),
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
            hint: styles.hint,
        },
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type to filter · [↑/↓] navigate · [Enter] select · [Esc] cancel",
        styles.hint,
    )));
    lines
}

/// The four steps answered before the wrap-up picker, restated above it.
///
/// Split out because it is shared by two tail steps: the wrap-up picker
/// itself, and `input_phoenix_lines` (the step after it, which appends its own
/// settled wrap_up_mode line on top of this). The sibling step renderers above
/// build their settled lines inline instead — none of them has a step after it
/// that needs to restate their answers.
fn answered_step_lines(app: &App, completed: Style) -> Vec<Line<'static>> {
    let summary = DraftSummary::from_input(&app.input);
    let draft = app.input.task_draft.as_ref();
    let repo_path = draft.map(|d| d.repo_path.clone()).unwrap_or_default();
    let base_branch = draft
        .map(|d| d.base_branch.clone())
        .unwrap_or_else(|| "main".to_string());
    vec![
        Line::from(Span::styled(
            format!("  Title: {}", summary.title),
            completed,
        )),
        Line::from(Span::styled(format!("  Tag: {}", summary.tag), completed)),
        Line::from(Span::styled(format!("  Repo: {repo_path}"), completed)),
        Line::from(Span::styled(
            format!("  Base branch: {base_branch}"),
            completed,
        )),
    ]
}

/// Close off a creation-form step page: the active line, a blank, the Esc hint.
///
/// A tail step is settled summaries, one active line, then this footer.
/// `active` is the part that differs per step — a styled span for a
/// single-key picker, a [`caret_field`] for a free-text one. The wrap-up and
/// phoenix pickers use it today; it is the shape a step added after them
/// would take.
fn form_step_page<'a>(mut settled: Vec<Line<'a>>, active: Line<'a>, hint: Style) -> Vec<Line<'a>> {
    settled.push(active);
    settled.push(Line::from(""));
    settled.push(Line::from(Span::styled("  [Esc] cancel", hint)));
    settled
}

pub(in crate::tui) fn input_wrap_up_mode_lines(
    app: &App,
    styles: &FormStyles,
) -> Vec<Line<'static>> {
    form_step_page(
        answered_step_lines(app, styles.completed),
        Line::from(Span::styled(
            "  Wrap-up: [r]ebase  [p]r  [d]one  [Enter] skip",
            styles.active,
        )),
        styles.hint,
    )
}

/// The number of lines [`input_phoenix_lines`] always returns: 5 settled
/// (title, tag, repo, base branch, wrap_up_mode) + active + blank + hint.
/// Draft content changes what those lines say, never how many there are, so
/// `input_panel_height` (kanban/mod.rs) uses this literal directly instead of
/// building the real `Vec` on every frame just to read its length.
/// `input_phoenix_lines_always_returns_a_fixed_line_count` below is what
/// keeps this pinned to the real count.
pub(in crate::tui) const PHOENIX_STEP_LINES: u16 = 8;

/// The form's last step (see docs/specs/tasks.allium's CreateTask guidance):
/// restates the five prior steps, including wrap_up_mode which is settled by
/// the time this step is active, then the phoenix y/N confirm.
pub(in crate::tui) fn input_phoenix_lines(app: &App, styles: &FormStyles) -> Vec<Line<'static>> {
    let mut settled = answered_step_lines(app, styles.completed);
    let wrap_up_mode = app
        .input
        .task_draft
        .as_ref()
        .and_then(|d| d.wrap_up_mode)
        .map(|m| m.to_string())
        .unwrap_or_else(|| "skip".to_string());
    settled.push(Line::from(Span::styled(
        format!("  Wrap-up: {wrap_up_mode}"),
        styles.completed,
    )));
    form_step_page(
        settled,
        Line::from(Span::styled(
            "  Phoenix — recreate this task when it's done? [y/N]",
            styles.active,
        )),
        styles.hint,
    )
}

fn repo_picker_lines<'a>(
    app: &'a App,
    area: Rect,
    header: &'a str,
    prefix: &'a str,
    hint_text: &'a str,
    styles: &FormStyles,
) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(Span::styled(header, styles.active)),
        Line::from(""),
        caret_field(&format!("  {prefix}: "), app, area, styles.active),
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
            hint: styles.hint,
        },
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(hint_text, styles.hint)));
    lines
}

pub(in crate::tui) fn main_session_dir_lines<'a>(
    app: &'a App,
    area: Rect,
    styles: &FormStyles,
) -> Vec<Line<'a>> {
    repo_picker_lines(
        app,
        area,
        "  Main session — base repo:",
        "Path",
        "  Type to filter · [↑/↓] navigate · [Enter] select · [Esc] cancel",
        styles,
    )
}

pub(in crate::tui) fn quick_dispatch_lines<'a>(
    app: &'a App,
    area: Rect,
    styles: &FormStyles,
) -> Vec<Line<'a>> {
    let filtered = crate::tui::filtered_repos(&app.board.repo_paths, &app.input.buffer);
    let mut lines = vec![
        Line::from(Span::styled(
            "  Quick Dispatch — select repo:",
            styles.active,
        )),
        Line::from(""),
        caret_field("  Filter: ", app, area, styles.active),
    ];
    append_filtered_repos_with_new_entry(
        &mut lines,
        &filtered,
        &app.input.buffer,
        app.input.repo_cursor,
        &RepoListCtx {
            height_offset: 7,
            area_height: area.height,
            hint: styles.hint,
        },
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type to filter · [↑/↓] navigate · [Enter] select · [Esc] cancel",
        styles.hint,
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
    styles: &FormStyles,
) -> Vec<Line<'static>> {
    vec![
        caret_field("  Title: ", app, area, styles.active),
        Line::from(""),
        Line::from(Span::styled("  [Enter] confirm  [Esc] cancel", styles.hint)),
    ]
}

pub(in crate::tui) fn input_epic_description_lines(
    app: &App,
    styles: &FormStyles,
) -> Vec<Line<'static>> {
    let title = app
        .input
        .epic_draft
        .as_ref()
        .map(|d| d.title.as_str())
        .unwrap_or("");
    vec![
        Line::from(Span::styled(format!("  Title: {title}"), styles.completed)),
        Line::from(Span::styled(
            "  Description: opening $EDITOR...".to_string(),
            styles.active,
        )),
        Line::from(""),
        Line::from(Span::styled("  [Esc] cancel", styles.hint)),
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

    // ---- DraftSummary -------------------------------------------------

    #[test]
    fn draft_summary_defaults_when_no_draft() {
        let input = InputState::default();
        let summary = DraftSummary::from_input(&input);

        assert_eq!(summary.title, "");
        assert_eq!(summary.tag, "none");
        assert_eq!(summary.description_oneline, "");
    }

    fn input_with_draft(draft: crate::tui::TaskDraft) -> InputState {
        InputState {
            task_draft: Some(draft),
            ..Default::default()
        }
    }

    #[test]
    fn draft_summary_tag_none_falls_back_to_none() {
        let input = input_with_draft(crate::tui::TaskDraft {
            tag: None,
            ..Default::default()
        });
        let summary = DraftSummary::from_input(&input);

        assert_eq!(summary.tag, "none");
    }

    #[test]
    fn draft_summary_tag_some_uses_display() {
        let input = input_with_draft(crate::tui::TaskDraft {
            tag: Some(crate::models::TaskTag::Bug),
            ..Default::default()
        });
        let summary = DraftSummary::from_input(&input);

        assert_eq!(summary.tag, crate::models::TaskTag::Bug.to_string());
        assert_ne!(summary.tag, "none");
    }

    #[test]
    fn draft_summary_oneline_equals_description_without_newline() {
        let input = input_with_draft(crate::tui::TaskDraft {
            description: "single line".to_string(),
            ..Default::default()
        });
        let summary = DraftSummary::from_input(&input);

        assert_eq!(summary.description_oneline, "single line");
    }

    #[test]
    fn draft_summary_oneline_truncates_multiline_with_ellipsis() {
        let input = input_with_draft(crate::tui::TaskDraft {
            description: "first line\nsecond line".to_string(),
            ..Default::default()
        });
        let summary = DraftSummary::from_input(&input);

        assert_eq!(summary.description_oneline, "first line ...");
    }

    #[test]
    fn draft_summary_title_reads_from_draft() {
        let input = input_with_draft(crate::tui::TaskDraft {
            title: "my task".to_string(),
            ..Default::default()
        });
        let summary = DraftSummary::from_input(&input);

        assert_eq!(summary.title, "my task");
    }

    // ---- input_phoenix_lines ----------------------------------------------

    fn form_styles() -> FormStyles {
        FormStyles {
            completed: Style::default(),
            active: Style::default(),
            hint: Style::default(),
        }
    }

    fn lines_text(lines: &[Line<'_>]) -> String {
        lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn input_phoenix_lines_restates_every_prior_step() {
        use crate::models::WrapUpMode;

        let mut app = crate::tui::App::new(vec![]);
        app.input.task_draft = Some(crate::tui::TaskDraft {
            title: "My task".to_string(),
            tag: Some(crate::models::TaskTag::Bug),
            repo_path: "/some/repo".to_string(),
            base_branch: "main".to_string(),
            wrap_up_mode: Some(WrapUpMode::Rebase),
            ..Default::default()
        });

        let text = lines_text(&input_phoenix_lines(&app, &form_styles()));

        assert!(text.contains("Title: My task"), "got:\n{text}");
        assert!(text.contains("Tag: bug"), "got:\n{text}");
        assert!(text.contains("Repo: /some/repo"), "got:\n{text}");
        assert!(text.contains("Base branch: main"), "got:\n{text}");
        assert!(text.contains("Wrap-up: rebase"), "got:\n{text}");
        assert!(text.contains("Phoenix"), "got:\n{text}");
        assert!(text.contains("[Esc] cancel"), "got:\n{text}");
    }

    #[test]
    fn input_phoenix_lines_shows_skip_when_wrap_up_mode_unset() {
        let mut app = crate::tui::App::new(vec![]);
        app.input.task_draft = Some(crate::tui::TaskDraft {
            wrap_up_mode: None,
            ..Default::default()
        });

        let text = lines_text(&input_phoenix_lines(&app, &form_styles()));

        assert!(text.contains("Wrap-up: skip"), "got:\n{text}");
    }

    #[test]
    fn input_phoenix_lines_always_returns_a_fixed_line_count() {
        // PHOENIX_STEP_LINES (kanban/mod.rs) is a literal, not derived from
        // this function, precisely because the count below never moves —
        // draft content changes what a line says, never how many lines
        // input_phoenix_lines produces. This test is what would catch a
        // future step edit going out of sync with that literal.
        let empty_draft = crate::tui::App::new(vec![]);
        let mut full_draft = crate::tui::App::new(vec![]);
        full_draft.input.task_draft = Some(crate::tui::TaskDraft {
            title: "My task".to_string(),
            tag: Some(crate::models::TaskTag::Bug),
            repo_path: "/some/repo".to_string(),
            base_branch: "main".to_string(),
            wrap_up_mode: Some(crate::models::WrapUpMode::Rebase),
            ..Default::default()
        });

        for app in [&empty_draft, &full_draft] {
            let lines = input_phoenix_lines(app, &form_styles());
            assert_eq!(lines.len(), PHOENIX_STEP_LINES as usize);
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
