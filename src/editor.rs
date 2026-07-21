/// A parse failure surfaced by [`parse_editor_content`] /
/// [`parse_epic_editor_output`]. The runtime turns these into a status message
/// so the user knows their input was rejected rather than silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorParseError {
    /// The section name as it appears in the editor file (e.g. `"STATUS"`).
    pub field: &'static str,
    /// The raw user-typed value that failed to parse.
    pub raw: String,
    /// Human-readable explanation suitable for a status bar message.
    pub message: String,
}

impl std::fmt::Display for EditorParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

#[derive(Default)]
pub struct EditorFields {
    pub title: String,
    pub description: String,
    pub repo_path: String,
    /// Parsed task status, or `None` if the section was empty or its content
    /// failed to parse. Parse failures are recorded in `errors`.
    pub status: Option<crate::models::TaskStatus>,
    pub plan: String,
    /// Parsed tag, or `None` if the section was empty or unparseable. Parse
    /// failures are recorded in `errors`.
    pub tag: Option<crate::models::TaskTag>,
    pub base_branch: String,
    /// Parsed wrap-up mode, or `None` if the section was empty/absent or
    /// unparseable. `None` is applied as "clear" in `apply_task_editor_fields`.
    pub wrap_up_mode: Option<crate::models::WrapUpMode>,
    /// Raw URL string. Empty means "clear the url".
    pub url: String,
    /// Parsed url_type, or `None` if the section was empty/absent or
    /// unparseable (parse failures are recorded in `errors`). `None` means
    /// "infer from the url, or preserve the prior type if the url is unchanged"
    /// at apply time.
    pub url_type: Option<crate::models::UrlType>,
    pub errors: Vec<EditorParseError>,
}

use crate::models::{Epic, Learning, LearningKind, Task};
use crate::service::FieldUpdate;

#[derive(Default)]
pub struct EpicEditorFields {
    pub title: String,
    pub description: String,
    pub feed_command: String, // "" → Clear, non-empty → Set
    /// Parsed seconds, or `None` if the section was empty or unparseable.
    /// Parse failures are recorded in `errors`.
    pub feed_interval_secs: Option<i64>,
    pub errors: Vec<EditorParseError>,
}

/// Parse `--- SECTION ---` delimited text into a map of section name → content.
fn parse_sections(input: &str) -> std::collections::HashMap<&str, String> {
    let mut sections = std::collections::HashMap::new();
    let mut current_section: Option<&str> = None;
    let mut current_buf = String::new();

    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("--- ") && trimmed.ends_with(" ---") {
            if let Some(name) = current_section {
                sections.insert(name, current_buf.trim().to_string());
            }
            let section = trimmed.trim_start_matches("--- ").trim_end_matches(" ---");
            current_section = Some(section);
            current_buf = String::new();
            continue;
        }
        if current_section.is_some() {
            if !current_buf.is_empty() {
                current_buf.push('\n');
            }
            current_buf.push_str(line);
        }
    }
    if let Some(name) = current_section {
        sections.insert(name, current_buf.trim().to_string());
    }
    sections
}

pub fn format_description_for_editor(existing: &str) -> String {
    format!("--- DESCRIPTION ---\n{existing}\n")
}

pub fn parse_description_editor_output(input: &str) -> String {
    let mut s = parse_sections(input);
    s.remove("DESCRIPTION").unwrap_or_default()
}

pub fn format_epic_for_editor(epic: &Epic) -> String {
    let feed_cmd = epic.feed_command.as_deref().unwrap_or("");
    let feed_interval = epic
        .feed_interval_secs
        .map(|n| n.to_string())
        .unwrap_or_default();
    format!(
        "--- TITLE ---\n{}\n--- DESCRIPTION ---\n{}\n--- FEED_COMMAND ---\n{}\n--- FEED_INTERVAL_SECS ---\n{}\n",
        epic.title, epic.description, feed_cmd, feed_interval
    )
}

/// Parse a section's raw value with `parser`. Empty input is treated as
/// "section absent" and returns `None` without an error. A non-empty value
/// that fails to parse pushes an [`EditorParseError`] onto `errors` and
/// returns `None`.
fn parse_section<T>(
    raw: String,
    field: &'static str,
    parser: impl FnOnce(&str) -> Option<T>,
    on_fail_message: impl FnOnce(&str) -> String,
    errors: &mut Vec<EditorParseError>,
) -> Option<T> {
    if raw.is_empty() {
        return None;
    }
    match parser(&raw) {
        Some(v) => Some(v),
        None => {
            let message = on_fail_message(&raw);
            errors.push(EditorParseError {
                field,
                raw,
                message,
            });
            None
        }
    }
}

pub fn parse_epic_editor_output(input: &str) -> EpicEditorFields {
    let mut s = parse_sections(input);
    let mut errors = Vec::new();
    let feed_interval_secs = parse_section(
        s.remove("FEED_INTERVAL_SECS").unwrap_or_default(),
        "FEED_INTERVAL_SECS",
        |raw| raw.parse::<i64>().ok(),
        |raw| format!("not a valid integer: {raw:?}"),
        &mut errors,
    );
    EpicEditorFields {
        title: s.remove("TITLE").unwrap_or_default(),
        description: s.remove("DESCRIPTION").unwrap_or_default(),
        feed_command: s.remove("FEED_COMMAND").unwrap_or_default(),
        feed_interval_secs,
        errors,
    }
}

pub fn format_editor_content(task: &Task) -> String {
    let plan = task.plan_path.as_deref().unwrap_or("");
    let tag = task.tag.map(|t| t.as_str()).unwrap_or("");
    let wrap_up_mode = task.wrap_up_mode.map(|m| m.as_str()).unwrap_or("");
    let url = task.url.as_ref().map(|u| u.url.as_str()).unwrap_or("");
    let url_type = task.url.as_ref().map(|u| u.url_type.as_str()).unwrap_or("");
    format!(
        "--- TITLE ---\n{title}\n\
         --- DESCRIPTION ---\n{description}\n\
         --- REPO_PATH ---\n{repo_path}\n\
         --- STATUS ---\n{status}\n\
         --- PLAN ---\n{plan}\n\
         --- TAG ---\n{tag}\n\
         --- BASE_BRANCH ---\n{base_branch}\n\
         --- WRAP_UP_MODE ---\n{wrap_up_mode}\n\
         --- URL ---\n{url}\n\
         --- URL_TYPE ---\n{url_type}\n",
        title = task.title,
        description = task.description,
        repo_path = task.repo_path,
        status = task.status.as_str(),
        base_branch = task.base_branch,
    )
}

/// Result of merging editor output with an existing [`Task`].
///
/// The "absent" convention is encoded in the field *type* rather than in
/// scattered empty-string checks, so each field's intent is explicit:
///
/// - **Non-nullable, keep-prior** (`title`, `description`, `repo_path`):
///   plain `String` already resolved against the prior value — an empty
///   section yields the task's prior value.
/// - **`status`**: a resolved [`TaskStatus`](crate::models::TaskStatus);
///   invalid/empty input falls back to the prior status.
/// - **`base_branch`**: `Option<String>` where `None` = keep the prior value
///   (the column is non-nullable, so it is never cleared from the editor).
/// - **Clearable fields** (`plan_path`, `tag`, `wrap_up_mode`): the editor
///   always states a definite intent (the section is always present), so an
///   empty section means *clear* and a filled section means *set*. `plan_path`
///   uses [`FieldUpdate`] (`Set`/`Clear`); `tag`/`wrap_up_mode` use `Option`
///   where `None` = clear.
/// - **`url`**: `Option<`[`UrlUpdate`](crate::service::UrlUpdate)`>` — `None`
///   leaves the field untouched (the edited url equals the prior url, a no-op);
///   `Some(Set/Clear)` is forwarded to the service only when it differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEditApplied {
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub status: crate::models::TaskStatus,
    pub plan_path: FieldUpdate,
    /// Post-edit plan path resolved once here, so callers building an
    /// in-memory snapshot (e.g. `finalize_task_edit`) consume it directly
    /// rather than re-deriving it from `plan_path`.
    pub resolved_plan_path: Option<String>,
    pub tag: Option<crate::models::TaskTag>,
    pub base_branch: Option<String>,
    pub wrap_up_mode: Option<crate::models::WrapUpMode>,
    pub url: Option<crate::service::UrlUpdate>,
    /// Post-edit url resolved once here (the value `url` diffs against the
    /// prior task to decide whether a DB write is needed), so callers get
    /// the true post-edit value even when `url` itself is `None` (no-op diff).
    pub resolved_url: Option<crate::models::TaskUrl>,
}

/// Resolve the desired `Option<TaskUrl>` from the parsed URL string and
/// (already-parsed) url_type, given the task's prior url. An empty url clears
/// the field. A present url with no explicit type preserves the prior type when
/// the url is unchanged (so a `security_alert` is never downgraded — `infer`
/// can only yield Pr/Issue/Other), otherwise infers the type from the url.
fn resolve_edited_url(
    raw_url: &str,
    explicit_type: Option<crate::models::UrlType>,
    prior: Option<&crate::models::TaskUrl>,
) -> Option<crate::models::TaskUrl> {
    use crate::models::{TaskUrl, UrlType};

    if raw_url.is_empty() {
        return None;
    }

    let url_type = explicit_type.unwrap_or_else(|| match prior {
        Some(p) if p.url == raw_url => p.url_type,
        _ => UrlType::infer(raw_url),
    });

    Some(TaskUrl::new(raw_url.to_string(), url_type))
}

/// Return `edited` unless it is empty, in which case fall back to `prior`.
/// The keep-prior convention for the non-nullable string fields.
fn keep_prior_if_empty(edited: String, prior: &str) -> String {
    if edited.is_empty() {
        prior.to_string()
    } else {
        edited
    }
}

/// Apply parsed editor fields on top of the task's existing values using
/// the rules documented in `tasks.allium::EditTask`.
pub fn apply_task_editor_fields(task: &Task, fields: EditorFields) -> TaskEditApplied {
    // Non-nullable keep-prior fields: an empty section restores the prior value.
    let title = keep_prior_if_empty(fields.title, &task.title);
    let description = keep_prior_if_empty(fields.description, &task.description);
    let repo_path = keep_prior_if_empty(fields.repo_path, &task.repo_path);
    // None covers both empty-section and unparseable input — both fall back
    // to the prior value. Unparseable input is also surfaced via
    // `fields.errors` for the runtime to render as a status message.
    let status = fields.status.unwrap_or(task.status);
    // Clearable plan: empty section clears, filled section sets.
    let plan_path = FieldUpdate::from_string(fields.plan);
    // Clearable tag/wrap_up_mode: `None` (empty/unparseable section) clears.
    let tag = fields.tag;
    let wrap_up_mode = fields.wrap_up_mode;
    // base_branch is non-nullable: empty preserves the prior value (`None`
    // tells the service not to touch the column).
    let base_branch = if fields.base_branch.is_empty() {
        None
    } else {
        Some(fields.base_branch)
    };
    // Diff the desired url against the prior so an unchanged edit is a true
    // no-op (no spurious write, no `was_pr_finalisation` read).
    let desired_url = resolve_edited_url(&fields.url, fields.url_type, task.url.as_ref());
    let url = if desired_url == task.url {
        None
    } else if let Some(ref u) = desired_url {
        Some(crate::service::UrlUpdate::Set(u.clone()))
    } else {
        Some(crate::service::UrlUpdate::Clear)
    };
    let resolved_plan_path = plan_path.as_option().map(str::to_string);
    TaskEditApplied {
        title,
        description,
        repo_path,
        status,
        plan_path,
        resolved_plan_path,
        tag,
        base_branch,
        wrap_up_mode,
        url,
        resolved_url: desired_url,
    }
}

/// Result of merging editor output with an existing [`Epic`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicEditApplied {
    pub title: String,
    pub description: String,
    pub feed_command: FieldUpdate,
    pub feed_interval_secs: Option<i64>,
}

/// Apply parsed epic editor fields on top of the epic's existing values.
/// Empty fields preserve the prior value.
pub fn apply_epic_editor_fields(epic: &Epic, fields: EpicEditorFields) -> EpicEditApplied {
    EpicEditApplied {
        title: if fields.title.is_empty() {
            epic.title.clone()
        } else {
            fields.title
        },
        description: if fields.description.is_empty() {
            epic.description.clone()
        } else {
            fields.description
        },
        feed_command: FieldUpdate::from_string(fields.feed_command),
        feed_interval_secs: fields.feed_interval_secs,
    }
}

pub fn parse_editor_content(input: &str) -> EditorFields {
    let mut s = parse_sections(input);
    let mut errors = Vec::new();

    let status = parse_section(
        s.remove("STATUS").unwrap_or_default(),
        "STATUS",
        crate::models::TaskStatus::parse,
        |raw| format!("unknown status: {raw:?}"),
        &mut errors,
    );

    let tag = parse_section(
        s.remove("TAG").unwrap_or_default(),
        "TAG",
        crate::models::TaskTag::parse,
        |raw| format!("unknown tag: {raw:?}"),
        &mut errors,
    );

    let wrap_up_mode = parse_section(
        s.remove("WRAP_UP_MODE").unwrap_or_default(),
        "WRAP_UP_MODE",
        crate::models::WrapUpMode::parse,
        |raw| format!("unknown wrap-up mode: {raw:?} (valid: rebase, pr, done)"),
        &mut errors,
    );

    let url_type = parse_section(
        s.remove("URL_TYPE").unwrap_or_default(),
        "URL_TYPE",
        crate::models::UrlType::parse,
        |raw| format!("unknown url type: {raw:?} (valid: pr, security_alert, issue, other)"),
        &mut errors,
    );

    EditorFields {
        title: s.remove("TITLE").unwrap_or_default(),
        description: s.remove("DESCRIPTION").unwrap_or_default(),
        repo_path: s.remove("REPO_PATH").unwrap_or_default(),
        status,
        plan: s.remove("PLAN").unwrap_or_default(),
        tag,
        base_branch: s.remove("BASE_BRANCH").unwrap_or_default(),
        wrap_up_mode,
        url: s.remove("URL").unwrap_or_default(),
        url_type,
        errors,
    }
}

pub struct LearningEditorFields {
    pub summary: String,
    pub kind: Option<LearningKind>,
    /// `None` = section absent (don't change).
    /// `Some(FieldUpdate::Clear)` = section present but empty (clear to NULL).
    /// `Some(FieldUpdate::Set(v))` = section present with text (set value).
    pub detail: Option<FieldUpdate>,
    /// `None` = section absent (don't change). `Some(vec![])` = clear tags.
    pub tags: Option<Vec<String>>,
}

pub fn format_learning_for_editor(learning: &Learning) -> String {
    let tags = learning.tags.join(", ");
    let detail = learning.detail.as_deref().unwrap_or("");
    format!(
        "--- SUMMARY ---\n{}\n--- KIND ---\n{}\n--- TAGS ---\n{}\n--- DETAIL ---\n{}\n",
        learning.summary,
        learning.kind.as_str(),
        tags,
        detail,
    )
}

pub fn parse_learning_editor_output(input: &str) -> LearningEditorFields {
    let mut s = parse_sections(input);
    let summary = s.remove("SUMMARY").unwrap_or_default();
    let kind = s.remove("KIND").and_then(|k| LearningKind::parse(&k));
    let tags = s.remove("TAGS").map(|t| {
        if t.trim().is_empty() {
            vec![]
        } else {
            t.split(',')
                .map(|tag| tag.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        }
    });
    let detail = s.remove("DETAIL").map(|d| {
        if d.trim().is_empty() {
            FieldUpdate::Clear
        } else {
            FieldUpdate::Set(d)
        }
    });
    LearningEditorFields {
        summary,
        kind,
        tags,
        detail,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::models::{EpicId, TaskId, TaskStatus};
    use chrono::Utc;
    use proptest::prelude::*;

    const FEED_INTERVAL_SLOW_SECS: i64 = 300;
    const FEED_INTERVAL_MED_SECS: i64 = 120;
    const FEED_INTERVAL_FAST_SECS: i64 = 60;

    fn make_epic(title: &str, description: &str) -> Epic {
        Epic {
            id: EpicId(1),
            title: title.to_string(),
            description: description.to_string(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: true,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn epic_editor_roundtrip_basic() {
        let epic = make_epic("My Epic", "A description");
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        assert_eq!(fields.title, "My Epic");
        assert_eq!(fields.description, "A description");
    }

    #[test]
    fn epic_editor_roundtrip_multiline_description() {
        let epic = make_epic("Title", "Line 1\nLine 2\nLine 3");
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        assert_eq!(fields.description, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn epic_editor_roundtrip_colons_in_title() {
        let epic = make_epic("Fix: auth system", "desc");
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        assert_eq!(fields.title, "Fix: auth system");
    }

    #[test]
    fn epic_editor_unknown_section_ignored() {
        let input = "--- TITLE ---\nHello\n--- UNKNOWN ---\nStuff\n--- DESCRIPTION ---\nmy desc\n";
        let fields = parse_epic_editor_output(input);
        assert_eq!(fields.title, "Hello");
        assert_eq!(fields.description, "my desc");
    }

    #[test]
    fn epic_editor_empty_input() {
        let fields = parse_epic_editor_output("");
        assert_eq!(fields.title, "");
        assert_eq!(fields.description, "");
    }

    fn make_task(
        title: &str,
        description: &str,
        repo_path: &str,
        status: TaskStatus,
        plan: Option<&str>,
    ) -> Task {
        Task {
            id: TaskId(1),
            title: title.to_string(),
            description: description.to_string(),
            repo_path: repo_path.to_string(),
            status,
            worktree: None,
            tmux_window: None,
            plan_path: plan.map(|s| s.to_string()),
            epic_id: None,
            sub_status: crate::models::SubStatus::None,
            url: None,
            tag: None,
            sort_order: None,
            base_branch: "main".into(),
            external_id: None,
            labels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_pre_tool_use_at: None,
            last_notification_at: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        }
    }

    #[test]
    fn editor_roundtrip_basic() {
        let task = make_task(
            "My Task",
            "A description",
            "/repo",
            TaskStatus::Backlog,
            Some("docs/plan.md"),
        );
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.title, "My Task");
        assert_eq!(fields.description, "A description");
        assert_eq!(fields.repo_path, "/repo");
        assert_eq!(fields.status, Some(TaskStatus::Backlog));
        assert_eq!(fields.plan, "docs/plan.md");
    }

    #[test]
    fn format_editor_content_maps_each_field_to_its_own_section() {
        // Regression guard for the positional-format fragility: each field
        // must land under its own section header, not a neighbour's. A
        // reordering mistake in the format! call would move a value to the
        // wrong section and this test would catch it directly (without going
        // through parse, which would silently "fix" a swap between
        // same-shaped string fields).
        let mut task = make_task(
            "field-title",
            "field-description",
            "field-repo-path",
            TaskStatus::Running,
            Some("field-plan"),
        );
        task.tag = Some(crate::models::TaskTag::Bug);
        task.base_branch = "field-base-branch".into();
        task.wrap_up_mode = Some(crate::models::WrapUpMode::Pr);
        task.url = Some(crate::models::TaskUrl::new(
            "field-url",
            crate::models::UrlType::Issue,
        ));

        let content = format_editor_content(&task);
        let sections = parse_sections(&content);

        assert_eq!(sections.get("TITLE").map(String::as_str), Some("field-title"));
        assert_eq!(
            sections.get("DESCRIPTION").map(String::as_str),
            Some("field-description")
        );
        assert_eq!(
            sections.get("REPO_PATH").map(String::as_str),
            Some("field-repo-path")
        );
        assert_eq!(sections.get("STATUS").map(String::as_str), Some("running"));
        assert_eq!(sections.get("PLAN").map(String::as_str), Some("field-plan"));
        assert_eq!(sections.get("TAG").map(String::as_str), Some("bug"));
        assert_eq!(
            sections.get("BASE_BRANCH").map(String::as_str),
            Some("field-base-branch")
        );
        assert_eq!(
            sections.get("WRAP_UP_MODE").map(String::as_str),
            Some("pr")
        );
        assert_eq!(sections.get("URL").map(String::as_str), Some("field-url"));
        assert_eq!(
            sections.get("URL_TYPE").map(String::as_str),
            Some("issue")
        );
    }

    #[test]
    fn editor_includes_base_branch() {
        let task = make_task("T", "D", "/repo", TaskStatus::Backlog, None);
        let content = format_editor_content(&task);
        assert!(
            content.contains("--- BASE_BRANCH ---"),
            "should have BASE_BRANCH section"
        );
        assert!(content.contains("main"), "should contain the branch value");
    }

    #[test]
    fn editor_roundtrip_base_branch() {
        let mut task = make_task("T", "D", "/repo", TaskStatus::Backlog, None);
        task.base_branch = "develop".into();
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.base_branch, "develop");
    }

    #[test]
    fn parse_base_branch_from_editor_output() {
        let input = "--- TITLE ---\nT\n--- BASE_BRANCH ---\nstaging\n";
        let fields = parse_editor_content(input);
        assert_eq!(fields.base_branch, "staging");
    }

    #[test]
    fn parse_base_branch_missing_returns_empty() {
        let input = "--- TITLE ---\nT\n--- STATUS ---\nbacklog\n";
        let fields = parse_editor_content(input);
        assert_eq!(fields.base_branch, "");
    }

    #[test]
    fn editor_roundtrip_colons_in_title() {
        let task = make_task("Fix: auth bug", "desc", "/repo", TaskStatus::Backlog, None);
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.title, "Fix: auth bug");
    }

    #[test]
    fn editor_roundtrip_colons_in_description() {
        let task = make_task(
            "Title",
            "Step 1: do this\nStep 2: do that",
            "/repo",
            TaskStatus::Backlog,
            None,
        );
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.description, "Step 1: do this\nStep 2: do that");
    }

    #[test]
    fn editor_multiline_description() {
        let task = make_task(
            "Title",
            "Line 1\nLine 2\nLine 3",
            "/repo",
            TaskStatus::Done,
            None,
        );
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.description, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn editor_unknown_section_ignored() {
        let input = "--- TITLE ---\nHello\n--- UNKNOWN ---\nStuff\n--- STATUS ---\nbacklog\n";
        let fields = parse_editor_content(input);
        assert_eq!(fields.title, "Hello");
        assert_eq!(fields.status, Some(TaskStatus::Backlog));
    }

    #[test]
    fn description_editor_roundtrip_empty() {
        let content = format_description_for_editor("");
        let result = parse_description_editor_output(&content);
        assert_eq!(result, "");
    }

    #[test]
    fn description_editor_roundtrip_multiline() {
        let content = format_description_for_editor("Line 1\nLine 2\nLine 3");
        let result = parse_description_editor_output(&content);
        assert_eq!(result, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn description_editor_roundtrip_with_dashes() {
        let content = format_description_for_editor("Some --- dashes --- in text");
        let result = parse_description_editor_output(&content);
        assert_eq!(result, "Some --- dashes --- in text");
    }

    proptest! {
        #[test]
        fn parse_editor_content_never_panics(input in "\\PC{0,2000}") {
            // parse_editor_content should never panic on arbitrary input
            let _ = parse_editor_content(&input);
        }
    }

    // --- apply_task_editor_fields -----------------------------------------

    fn sample_task() -> Task {
        let mut t = make_task(
            "Original title",
            "Original description",
            "/orig/repo",
            TaskStatus::Running,
            Some("docs/plan.md"),
        );
        t.tag = Some(crate::models::TaskTag::Bug);
        t.base_branch = "develop".into();
        t
    }

    #[test]
    fn apply_task_editor_fields_roundtrip() {
        let task = sample_task();
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.title, task.title);
        assert_eq!(applied.description, task.description);
        assert_eq!(applied.repo_path, task.repo_path);
        assert_eq!(applied.status, task.status);
        assert_eq!(
            applied.plan_path,
            FieldUpdate::Set(task.plan_path.clone().unwrap())
        );
        assert_eq!(applied.tag, task.tag);
        assert_eq!(
            applied.base_branch.as_deref(),
            Some(task.base_branch.as_str())
        );
    }

    #[test]
    fn apply_task_empty_title_preserves_prior() {
        let task = sample_task();
        let fields = EditorFields {
            description: "New desc".into(),
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.title, "Original title");
        assert_eq!(applied.description, "New desc");
        // empty repo_path preserves prior
        assert_eq!(applied.repo_path, "/orig/repo");
    }

    #[test]
    fn editor_includes_wrap_up_mode_section() {
        let mut task = make_task("T", "D", "/repo", TaskStatus::Backlog, None);
        task.wrap_up_mode = Some(crate::models::WrapUpMode::Rebase);
        let content = format_editor_content(&task);
        assert!(
            content.contains("--- WRAP_UP_MODE ---"),
            "should have WRAP_UP_MODE section: {content}"
        );
        assert!(
            content.contains("rebase"),
            "should contain wrap-up mode value"
        );
    }

    #[test]
    fn editor_roundtrip_wrap_up_mode() {
        let mut task = make_task("T", "D", "/repo", TaskStatus::Backlog, None);
        task.wrap_up_mode = Some(crate::models::WrapUpMode::Pr);
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.wrap_up_mode, Some(crate::models::WrapUpMode::Pr));
    }

    #[test]
    fn editor_wrap_up_mode_none_when_empty() {
        let input = "--- TITLE ---\nT\n--- WRAP_UP_MODE ---\n\n";
        let fields = parse_editor_content(input);
        assert_eq!(fields.wrap_up_mode, None);
    }

    #[test]
    fn apply_task_editor_wrap_up_mode_set() {
        let task = make_task("T", "D", "/repo", TaskStatus::Backlog, None);
        let fields = EditorFields {
            title: "T".into(),
            wrap_up_mode: Some(crate::models::WrapUpMode::Done),
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.wrap_up_mode, Some(crate::models::WrapUpMode::Done));
    }

    #[test]
    fn apply_task_editor_wrap_up_mode_clear() {
        let mut task = make_task("T", "D", "/repo", TaskStatus::Backlog, None);
        task.wrap_up_mode = Some(crate::models::WrapUpMode::Rebase);
        let fields = EditorFields {
            title: "T".into(),
            wrap_up_mode: None,
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(
            applied.wrap_up_mode, None,
            "empty wrap_up_mode section should clear it"
        );
    }

    #[test]
    fn apply_task_editor_fields_resolved_plan_path_matches_set() {
        let task = sample_task();
        let fields = EditorFields {
            plan: "docs/new-plan.md".into(),
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.plan_path, FieldUpdate::Set("docs/new-plan.md".into()));
        assert_eq!(
            applied.resolved_plan_path.as_deref(),
            Some("docs/new-plan.md")
        );
    }

    #[test]
    fn apply_task_editor_fields_resolved_plan_path_none_when_cleared() {
        let task = sample_task();
        let fields = EditorFields::default();
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.plan_path, FieldUpdate::Clear);
        assert_eq!(applied.resolved_plan_path, None);
    }

    #[test]
    fn apply_task_empty_plan_clears_plan() {
        let task = sample_task();
        assert!(task.plan_path.is_some());
        let fields = EditorFields {
            tag: Some(crate::models::TaskTag::Bug),
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(
            applied.plan_path,
            FieldUpdate::Clear,
            "empty plan should clear plan"
        );
    }

    #[test]
    fn apply_task_empty_tag_clears_tag() {
        let task = sample_task();
        let fields = EditorFields::default();
        let applied = apply_task_editor_fields(&task, fields);
        assert!(applied.tag.is_none());
    }

    #[test]
    fn parse_invalid_status_records_error_and_preserves_prior_on_apply() {
        let input = "--- TITLE ---\nT\n--- STATUS ---\nnonsense\n";
        let parsed = parse_editor_content(input);
        assert!(parsed.status.is_none());
        assert_eq!(parsed.errors.len(), 1);
        assert_eq!(parsed.errors[0].field, "STATUS");
        assert!(parsed.errors[0].message.contains("nonsense"));

        let task = sample_task();
        let applied = apply_task_editor_fields(&task, parsed);
        assert_eq!(applied.status, task.status);
    }

    #[test]
    fn parse_invalid_tag_records_error_and_clears_on_apply() {
        let input = "--- TITLE ---\nT\n--- TAG ---\nnot-a-real-tag\n";
        let parsed = parse_editor_content(input);
        assert!(parsed.tag.is_none());
        assert_eq!(parsed.errors.len(), 1);
        assert_eq!(parsed.errors[0].field, "TAG");

        let task = sample_task();
        let applied = apply_task_editor_fields(&task, parsed);
        assert!(applied.tag.is_none());
    }

    #[test]
    fn parse_valid_input_has_no_errors() {
        let task = sample_task();
        let parsed = parse_editor_content(&format_editor_content(&task));
        assert!(parsed.errors.is_empty(), "got: {:?}", parsed.errors);
        assert_eq!(parsed.status, Some(task.status));
        assert_eq!(parsed.tag, task.tag);
    }

    #[test]
    fn apply_task_empty_base_branch_preserves_prior() {
        let task = sample_task();
        let fields = EditorFields::default();
        let applied = apply_task_editor_fields(&task, fields);
        // When the field is "" we preserve by returning None so the DB
        // patch does not touch it. The runtime's merger then keeps the
        // prior value.
        assert!(applied.base_branch.is_none());
    }

    #[test]
    fn apply_task_unchanged_content_yields_same_task() {
        // Regression guard: if a user opens the editor and closes without
        // changes, the applied result must equal the original task's values.
        let task = sample_task();
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.title, task.title);
        assert_eq!(applied.description, task.description);
        assert_eq!(applied.repo_path, task.repo_path);
        assert_eq!(applied.status, task.status);
        assert_eq!(
            applied.plan_path,
            FieldUpdate::Set(task.plan_path.clone().unwrap())
        );
        assert_eq!(applied.tag, task.tag);
        // No url on the task → no url change requested.
        assert_eq!(applied.url, None);
    }

    // --- url editing ------------------------------------------------------

    use crate::models::{TaskUrl, UrlType};
    use crate::service::UrlUpdate;

    fn sample_task_with_url(url: TaskUrl) -> Task {
        let mut t = sample_task();
        t.url = Some(url);
        t
    }

    #[test]
    fn editor_includes_url_sections() {
        let task = sample_task_with_url(TaskUrl::new("https://github.com/o/r/pull/9", UrlType::Pr));
        let content = format_editor_content(&task);
        assert!(content.contains("--- URL ---"), "{content}");
        assert!(content.contains("--- URL_TYPE ---"), "{content}");
        assert!(content.contains("https://github.com/o/r/pull/9"));
        assert!(content.contains("pr"));
    }

    #[test]
    fn url_roundtrip_no_url_is_noop() {
        let task = sample_task();
        let fields = parse_editor_content(&format_editor_content(&task));
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.url, None);
    }

    #[test]
    fn url_roundtrip_pr_is_noop() {
        let task =
            sample_task_with_url(TaskUrl::new("https://github.com/o/r/pull/42", UrlType::Pr));
        let fields = parse_editor_content(&format_editor_content(&task));
        let applied = apply_task_editor_fields(&task, fields);
        // Unchanged edit → no url update.
        assert_eq!(applied.url, None);
    }

    #[test]
    fn url_roundtrip_security_alert_preserves_type() {
        // A security_alert url whose string contains neither /pull/ nor
        // /issues/ would infer to Other — round-trip must keep security_alert.
        let task = sample_task_with_url(TaskUrl::new(
            "https://github.com/o/r/security/dependabot/3",
            UrlType::SecurityAlert,
        ));
        let fields = parse_editor_content(&format_editor_content(&task));
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.url, None, "unchanged round-trip must be a no-op");
    }

    #[test]
    fn url_security_alert_type_preserved_when_type_section_cleared() {
        // User blanks URL_TYPE but leaves the (unchanged) security-alert url.
        // The prior type must be preserved, not inferred down to Other.
        let url = TaskUrl::new(
            "https://github.com/o/r/security/dependabot/3",
            UrlType::SecurityAlert,
        );
        let task = sample_task_with_url(url.clone());
        let fields = EditorFields {
            url: url.url.clone(),
            url_type: None, // section cleared
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        // url unchanged → no-op, and the type stayed security_alert.
        assert_eq!(applied.url, None);
    }

    #[test]
    fn url_empty_clears_existing_url() {
        let task = sample_task_with_url(TaskUrl::new("https://github.com/o/r/pull/1", UrlType::Pr));
        let fields = EditorFields {
            url: String::new(),
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.url, Some(UrlUpdate::Clear));
    }

    #[test]
    fn url_empty_on_task_without_url_is_noop() {
        let task = sample_task();
        let fields = EditorFields {
            url: String::new(),
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.url, None);
    }

    #[test]
    fn apply_task_editor_fields_resolved_url_reflects_new_value_even_when_unchanged() {
        // resolved_url must reflect the post-edit url regardless of whether
        // `url` itself is None (unchanged, no diff to forward to the DB).
        let task = sample_task_with_url(TaskUrl::new("https://github.com/o/r/pull/9", UrlType::Pr));
        let fields = parse_editor_content(&format_editor_content(&task));
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.url, None, "unchanged edit is a no-op diff");
        assert_eq!(
            applied.resolved_url,
            Some(TaskUrl::new("https://github.com/o/r/pull/9", UrlType::Pr)),
            "resolved_url must still carry the post-edit value"
        );
    }

    #[test]
    fn apply_task_editor_fields_resolved_url_none_when_cleared() {
        let task = sample_task_with_url(TaskUrl::new("https://github.com/o/r/pull/1", UrlType::Pr));
        let fields = EditorFields {
            url: String::new(),
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(applied.url, Some(UrlUpdate::Clear));
        assert_eq!(applied.resolved_url, None);
    }

    #[test]
    fn url_new_with_empty_type_infers_pr() {
        let task = sample_task();
        let fields = EditorFields {
            url: "https://github.com/o/r/pull/7".into(),
            url_type: None,
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(
            applied.url,
            Some(UrlUpdate::Set(TaskUrl::new(
                "https://github.com/o/r/pull/7",
                UrlType::Pr
            )))
        );
    }

    #[test]
    fn url_new_with_empty_type_infers_issue() {
        let task = sample_task();
        let fields = EditorFields {
            url: "https://github.com/o/r/issues/7".into(),
            url_type: None,
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(
            applied.url,
            Some(UrlUpdate::Set(TaskUrl::new(
                "https://github.com/o/r/issues/7",
                UrlType::Issue
            )))
        );
    }

    #[test]
    fn url_explicit_type_wins() {
        let task = sample_task();
        let fields = EditorFields {
            url: "https://example.com/x".into(),
            url_type: Some(UrlType::SecurityAlert),
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(
            applied.url,
            Some(UrlUpdate::Set(TaskUrl::new(
                "https://example.com/x",
                UrlType::SecurityAlert
            )))
        );
    }

    #[test]
    fn parse_invalid_url_type_records_error_and_falls_back_to_infer() {
        let input = "--- URL ---\nhttps://github.com/o/r/pull/5\n--- URL_TYPE ---\nbogus\n";
        let parsed = parse_editor_content(input);
        assert!(parsed.url_type.is_none());
        assert_eq!(parsed.errors.len(), 1);
        assert_eq!(parsed.errors[0].field, "URL_TYPE");

        let task = sample_task();
        let applied = apply_task_editor_fields(&task, parsed);
        // url still applied, type inferred from the /pull/ path.
        assert_eq!(
            applied.url,
            Some(UrlUpdate::Set(TaskUrl::new(
                "https://github.com/o/r/pull/5",
                UrlType::Pr
            )))
        );
    }

    #[test]
    fn url_changed_string_reinfers_type() {
        // Prior type was security_alert; user replaces the url with a /pull/
        // link and blanks the type → infer pr (prior type must NOT stick to a
        // different url).
        let task = sample_task_with_url(TaskUrl::new(
            "https://github.com/o/r/security/dependabot/3",
            UrlType::SecurityAlert,
        ));
        let fields = EditorFields {
            url: "https://github.com/o/r/pull/9".into(),
            url_type: None,
            ..Default::default()
        };
        let applied = apply_task_editor_fields(&task, fields);
        assert_eq!(
            applied.url,
            Some(UrlUpdate::Set(TaskUrl::new(
                "https://github.com/o/r/pull/9",
                UrlType::Pr
            )))
        );
    }

    // --- apply_epic_editor_fields -----------------------------------------

    #[test]
    fn apply_epic_editor_fields_roundtrip() {
        let epic = make_epic("E title", "E desc");
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        let applied = apply_epic_editor_fields(&epic, fields);
        assert_eq!(applied.title, epic.title);
        assert_eq!(applied.description, epic.description);
        assert_eq!(applied.feed_command, FieldUpdate::Clear);
        assert_eq!(applied.feed_interval_secs, None);
    }

    #[test]
    fn apply_epic_empty_fields_preserve_prior() {
        let epic = make_epic("E title", "E desc");
        let fields = EpicEditorFields {
            description: "new desc".into(),
            ..Default::default()
        };
        let applied = apply_epic_editor_fields(&epic, fields);
        assert_eq!(applied.title, "E title");
        assert_eq!(applied.description, "new desc");
        assert_eq!(applied.feed_command, FieldUpdate::Clear);
        assert_eq!(applied.feed_interval_secs, None);
    }

    #[test]
    fn epic_editor_includes_feed_command_section() {
        let mut epic = make_epic("T", "D");
        epic.feed_command = Some("scripts/fetch-dependabot.sh".into());
        let content = format_epic_for_editor(&epic);
        assert!(content.contains("--- FEED_COMMAND ---"));
        assert!(content.contains("scripts/fetch-dependabot.sh"));
    }

    #[test]
    fn epic_editor_includes_feed_interval_section() {
        let mut epic = make_epic("T", "D");
        epic.feed_interval_secs = Some(FEED_INTERVAL_SLOW_SECS);
        let content = format_epic_for_editor(&epic);
        assert!(content.contains("--- FEED_INTERVAL_SECS ---"));
        assert!(content.contains("300"));
    }

    #[test]
    fn epic_editor_roundtrip_feed_command_set() {
        let mut epic = make_epic("T", "D");
        epic.feed_command = Some("my-script.sh".into());
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        assert_eq!(fields.feed_command, "my-script.sh");
    }

    #[test]
    fn epic_editor_roundtrip_feed_command_empty() {
        let epic = make_epic("T", "D");
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        assert_eq!(fields.feed_command, "");
    }

    #[test]
    fn epic_editor_roundtrip_feed_interval_set() {
        let mut epic = make_epic("T", "D");
        epic.feed_interval_secs = Some(FEED_INTERVAL_MED_SECS);
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        assert_eq!(fields.feed_interval_secs, Some(FEED_INTERVAL_MED_SECS));
        assert!(fields.errors.is_empty());
    }

    #[test]
    fn parse_epic_invalid_feed_interval_records_error() {
        let input = "--- TITLE ---\nT\n--- FEED_INTERVAL_SECS ---\nnot-a-number\n";
        let parsed = parse_epic_editor_output(input);
        assert_eq!(parsed.feed_interval_secs, None);
        assert_eq!(parsed.errors.len(), 1);
        assert_eq!(parsed.errors[0].field, "FEED_INTERVAL_SECS");
    }

    #[test]
    fn apply_epic_feed_command_set() {
        let epic = make_epic("T", "D");
        let fields = EpicEditorFields {
            title: "T".into(),
            description: "D".into(),
            feed_command: "my-script.sh".into(),
            feed_interval_secs: None,
            errors: Vec::new(),
        };
        let applied = apply_epic_editor_fields(&epic, fields);
        assert_eq!(
            applied.feed_command,
            crate::service::FieldUpdate::Set("my-script.sh".into())
        );
        assert_eq!(applied.feed_interval_secs, None);
    }

    #[test]
    fn apply_epic_feed_command_clear() {
        let mut epic = make_epic("T", "D");
        epic.feed_command = Some("old-script.sh".into());
        let fields = EpicEditorFields {
            title: "T".into(),
            description: "D".into(),
            feed_command: String::new(),
            feed_interval_secs: None,
            errors: Vec::new(),
        };
        let applied = apply_epic_editor_fields(&epic, fields);
        assert_eq!(applied.feed_command, crate::service::FieldUpdate::Clear);
    }

    #[test]
    fn apply_epic_feed_interval_valid() {
        let epic = make_epic("T", "D");
        let fields = EpicEditorFields {
            title: "T".into(),
            description: "D".into(),
            feed_command: String::new(),
            feed_interval_secs: Some(FEED_INTERVAL_SLOW_SECS),
            errors: Vec::new(),
        };
        let applied = apply_epic_editor_fields(&epic, fields);
        assert_eq!(applied.feed_interval_secs, Some(FEED_INTERVAL_SLOW_SECS));
    }

    #[test]
    fn apply_epic_editor_fields_full_roundtrip() {
        let mut epic = make_epic("E title", "E desc");
        epic.feed_command = Some("scripts/fetch-dependabot.sh".into());
        epic.feed_interval_secs = Some(FEED_INTERVAL_FAST_SECS);
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        let applied = apply_epic_editor_fields(&epic, fields);
        assert_eq!(applied.title, "E title");
        assert_eq!(
            applied.feed_command,
            crate::service::FieldUpdate::Set("scripts/fetch-dependabot.sh".into())
        );
        assert_eq!(applied.feed_interval_secs, Some(FEED_INTERVAL_FAST_SECS));
    }

    mod learning_editor_tests {
        use super::*;

        fn make_learning() -> Learning {
            use crate::models::{LearningId, LearningScope, LearningStatus};
            Learning {
                id: LearningId(1),
                kind: LearningKind::Convention,
                summary: "Use LearningService not raw db".to_string(),
                detail: Some("Ensures validation runs.".to_string()),
                scope: LearningScope::Repo,
                scope_ref: Some("/repo".to_string()),
                tags: vec!["arch".to_string(), "service".to_string()],
                status: LearningStatus::Approved,
                source_task_id: None,
                upvote_count: 0,
                last_upvoted_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }
        }

        #[test]
        fn format_round_trips_all_fields() {
            let l = make_learning();
            let s = format_learning_for_editor(&l);
            let f = parse_learning_editor_output(&s);
            assert_eq!(f.summary, l.summary);
            assert_eq!(f.kind, Some(l.kind));
            assert_eq!(f.tags, Some(l.tags.clone()));
            assert_eq!(f.detail, Some(FieldUpdate::Set(l.detail.clone().unwrap())));
        }

        #[test]
        fn format_round_trips_no_detail() {
            let mut l = make_learning();
            l.detail = None;
            let s = format_learning_for_editor(&l);
            let f = parse_learning_editor_output(&s);
            // Empty DETAIL section → Some(FieldUpdate::Clear) meaning "clear"
            assert_eq!(f.detail, Some(FieldUpdate::Clear));
        }

        #[test]
        fn parse_empty_summary_returns_empty_string() {
            let input =
                "--- SUMMARY ---\n\n--- KIND ---\nconvention\n--- TAGS ---\n\n--- DETAIL ---\n\n";
            let f = parse_learning_editor_output(input);
            assert_eq!(f.summary, "");
        }

        #[test]
        fn parse_unknown_kind_returns_none() {
            let input = "--- SUMMARY ---\nfoo\n--- KIND ---\nnot_a_kind\n--- TAGS ---\n\n--- DETAIL ---\n\n";
            let f = parse_learning_editor_output(input);
            assert_eq!(f.kind, None);
        }

        #[test]
        fn parse_missing_sections_return_none() {
            let f = parse_learning_editor_output("--- SUMMARY ---\nsome text\n");
            assert_eq!(f.kind, None);
            assert_eq!(f.tags, None);
            assert_eq!(f.detail, None);
        }

        #[test]
        fn parse_tags_trims_whitespace() {
            let input = "--- SUMMARY ---\nfoo\n--- KIND ---\npitfall\n--- TAGS ---\n async , rust \n--- DETAIL ---\n\n";
            let f = parse_learning_editor_output(input);
            assert_eq!(f.tags, Some(vec!["async".to_string(), "rust".to_string()]));
        }
    }
}
