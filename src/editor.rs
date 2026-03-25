pub struct EditorFields {
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub status: String,
}

pub fn format_editor_content(title: &str, description: &str, repo_path: &str, status: &str) -> String {
    format!(
        "--- TITLE ---\n{title}\n--- DESCRIPTION ---\n{description}\n--- REPO_PATH ---\n{repo_path}\n--- STATUS ---\n{status}\n"
    )
}

pub fn parse_editor_content(input: &str) -> EditorFields {
    let mut current_section: Option<&str> = None;
    let mut title = String::new();
    let mut description = String::new();
    let mut repo_path = String::new();
    let mut status = String::new();

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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_roundtrip_basic() {
        let content = format_editor_content("My Task", "A description", "/repo", "ready");
        let fields = parse_editor_content(&content);
        assert_eq!(fields.title, "My Task");
        assert_eq!(fields.description, "A description");
        assert_eq!(fields.repo_path, "/repo");
        assert_eq!(fields.status, "ready");
    }

    #[test]
    fn editor_roundtrip_colons_in_title() {
        let content = format_editor_content("Fix: auth bug", "desc", "/repo", "backlog");
        let fields = parse_editor_content(&content);
        assert_eq!(fields.title, "Fix: auth bug");
    }

    #[test]
    fn editor_roundtrip_colons_in_description() {
        let content = format_editor_content("Title", "Step 1: do this\nStep 2: do that", "/repo", "ready");
        let fields = parse_editor_content(&content);
        assert_eq!(fields.description, "Step 1: do this\nStep 2: do that");
    }

    #[test]
    fn editor_multiline_description() {
        let content = format_editor_content("Title", "Line 1\nLine 2\nLine 3", "/repo", "done");
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
