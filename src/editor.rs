#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorSection {
    Title,
    Description,
    Plan,
}

pub struct EditorFields {
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub status: String,
    pub plan: String,
    pub tag: String,
}

use crate::models::{Epic, Task};

pub struct EpicEditorFields {
    pub title: String,
    pub description: String,
    pub repo_path: String,
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

pub fn format_epic_for_editor(epic: &Epic) -> String {
    format!(
        "--- TITLE ---\n{}\n--- DESCRIPTION ---\n{}\n--- REPO_PATH ---\n{}\n",
        epic.title, epic.description, epic.repo_path
    )
}

pub fn parse_epic_editor_output(input: &str) -> EpicEditorFields {
    let mut s = parse_sections(input);
    EpicEditorFields {
        title: s.remove("TITLE").unwrap_or_default(),
        description: s.remove("DESCRIPTION").unwrap_or_default(),
        repo_path: s.remove("REPO_PATH").unwrap_or_default(),
    }
}

pub fn format_editor_content(task: &Task) -> String {
    let plan = task.plan.as_deref().unwrap_or("");
    let tag = task.tag.map(|t| t.as_str()).unwrap_or("");
    format!(
        "--- TITLE ---\n{}\n--- DESCRIPTION ---\n{}\n--- REPO_PATH ---\n{}\n--- STATUS ---\n{}\n--- PLAN ---\n{}\n--- TAG ---\n{}\n",
        task.title, task.description, task.repo_path, task.status.as_str(), plan, tag
    )
}

pub fn parse_editor_content(input: &str) -> EditorFields {
    let mut s = parse_sections(input);
    EditorFields {
        title: s.remove("TITLE").unwrap_or_default(),
        description: s.remove("DESCRIPTION").unwrap_or_default(),
        repo_path: s.remove("REPO_PATH").unwrap_or_default(),
        status: s.remove("STATUS").unwrap_or_default(),
        plan: s.remove("PLAN").unwrap_or_default(),
        tag: s.remove("TAG").unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EpicId, TaskId, TaskStatus};
    use chrono::Utc;
    use proptest::prelude::*;

    fn make_epic(title: &str, description: &str, repo_path: &str) -> Epic {
        Epic {
            id: EpicId(1),
            title: title.to_string(),
            description: description.to_string(),
            repo_path: repo_path.to_string(),
            status: TaskStatus::Backlog,
            plan: None,
            sort_order: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn epic_editor_roundtrip_basic() {
        let epic = make_epic("My Epic", "A description", "/repo");
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        assert_eq!(fields.title, "My Epic");
        assert_eq!(fields.description, "A description");
        assert_eq!(fields.repo_path, "/repo");
    }

    #[test]
    fn epic_editor_roundtrip_multiline_description() {
        let epic = make_epic("Title", "Line 1\nLine 2\nLine 3", "/repo");
        let content = format_epic_for_editor(&epic);
        let fields = parse_epic_editor_output(&content);
        assert_eq!(fields.description, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn epic_editor_roundtrip_colons_in_title() {
        let epic = make_epic("Fix: auth system", "desc", "/repo");
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
        assert_eq!(fields.repo_path, "");
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
            plan: plan.map(|s| s.to_string()),
            epic_id: None,
            sub_status: crate::models::SubStatus::None,
            pr_url: None,
            tag: None,
            sort_order: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
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
        assert_eq!(fields.status, "backlog");
        assert_eq!(fields.plan, "docs/plan.md");
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
        assert_eq!(fields.status, "backlog");
    }

    proptest! {
        #[test]
        fn parse_editor_content_never_panics(input in "\\PC{0,2000}") {
            // parse_editor_content should never panic on arbitrary input
            let _ = parse_editor_content(&input);
        }
    }
}
