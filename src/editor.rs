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
}

use crate::models::{Epic, Task};

pub struct EpicEditorFields {
    pub title: String,
    pub description: String,
    pub repo_path: String,
}

pub fn format_epic_for_editor(epic: &Epic) -> String {
    format!(
        "--- TITLE ---\n{}\n--- DESCRIPTION ---\n{}\n--- REPO_PATH ---\n{}\n",
        epic.title, epic.description, epic.repo_path
    )
}

pub fn parse_epic_editor_output(input: &str) -> EpicEditorFields {
    let mut current_section: Option<&str> = None;
    let mut title = String::new();
    let mut description = String::new();
    let mut repo_path = String::new();

    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("--- ") && trimmed.ends_with(" ---") {
            let section = trimmed.trim_start_matches("--- ").trim_end_matches(" ---");
            current_section = Some(section);
            continue;
        }
        let target = match current_section {
            Some("TITLE") => &mut title,
            Some("DESCRIPTION") => &mut description,
            Some("REPO_PATH") => &mut repo_path,
            _ => continue,
        };
        if !target.is_empty() {
            target.push('\n');
        }
        target.push_str(line);
    }

    EpicEditorFields {
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        repo_path: repo_path.trim().to_string(),
    }
}

pub fn format_editor_content(task: &Task) -> String {
    let plan = task.plan.as_deref().unwrap_or("");
    format!(
        "--- TITLE ---\n{}\n--- DESCRIPTION ---\n{}\n--- REPO_PATH ---\n{}\n--- STATUS ---\n{}\n--- PLAN ---\n{}\n",
        task.title, task.description, task.repo_path, task.status.as_str(), plan
    )
}

pub fn parse_editor_content(input: &str) -> EditorFields {
    let mut current_section: Option<&str> = None;
    let mut title = String::new();
    let mut description = String::new();
    let mut repo_path = String::new();
    let mut status = String::new();
    let mut plan = String::new();

    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("--- ") && trimmed.ends_with(" ---") {
            let section = trimmed.trim_start_matches("--- ").trim_end_matches(" ---");
            current_section = Some(section);
            continue;
        }
        let target = match current_section {
            Some("TITLE") => &mut title,
            Some("DESCRIPTION") => &mut description,
            Some("REPO_PATH") => &mut repo_path,
            Some("STATUS") => &mut status,
            Some("PLAN") => &mut plan,
            _ => continue,
        };
        if !target.is_empty() {
            target.push('\n');
        }
        target.push_str(line);
    }

    EditorFields {
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        repo_path: repo_path.trim().to_string(),
        status: status.trim().to_string(),
        plan: plan.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EpicId, TaskId, TaskStatus};
    use chrono::Utc;

    fn make_epic(title: &str, description: &str, repo_path: &str) -> Epic {
        Epic {
            id: EpicId(1),
            title: title.to_string(),
            description: description.to_string(),
            repo_path: repo_path.to_string(),
            done: false,
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

    fn make_task(title: &str, description: &str, repo_path: &str, status: TaskStatus, plan: Option<&str>) -> Task {
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
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn editor_roundtrip_basic() {
        let task = make_task("My Task", "A description", "/repo", TaskStatus::Ready, Some("docs/plan.md"));
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.title, "My Task");
        assert_eq!(fields.description, "A description");
        assert_eq!(fields.repo_path, "/repo");
        assert_eq!(fields.status, "ready");
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
        let task = make_task("Title", "Step 1: do this\nStep 2: do that", "/repo", TaskStatus::Ready, None);
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.description, "Step 1: do this\nStep 2: do that");
    }

    #[test]
    fn editor_multiline_description() {
        let task = make_task("Title", "Line 1\nLine 2\nLine 3", "/repo", TaskStatus::Done, None);
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.description, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn editor_unknown_section_ignored() {
        let input = "--- TITLE ---\nHello\n--- UNKNOWN ---\nStuff\n--- STATUS ---\nready\n";
        let fields = parse_editor_content(input);
        assert_eq!(fields.title, "Hello");
        assert_eq!(fields.status, "ready");
    }
}
