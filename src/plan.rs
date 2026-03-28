use anyhow::Result;

/// Metadata extracted from a plan markdown file.
#[derive(Debug, PartialEq)]
pub struct PlanMetadata {
    pub title: String,
    pub description: String,
}

/// Parse a plan markdown file and extract title and description.
///
/// - Title: first H1 heading, with trailing " — Implementation Plan" stripped
/// - Description: content of the first `**Goal:**` line, prefix removed
pub fn parse_plan(content: &str) -> Result<PlanMetadata> {
    let title = content
        .lines()
        .find(|line| line.starts_with("# "))
        .ok_or_else(|| anyhow::anyhow!("No H1 heading found. Use --title to provide a title manually."))?;

    let title = title
        .trim_start_matches('#')
        .trim()
        .trim_end_matches("\u{2014} Implementation Plan")
        .trim()
        .to_string();

    let description = content
        .lines()
        .find(|line| line.contains("**Goal:**"))
        .map(|line| {
            line.split("**Goal:**")
                .nth(1)
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .unwrap_or_default();

    Ok(PlanMetadata { title, description })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn parse_never_panics(input in "\\PC{0,2000}") {
            // parse_plan should never panic on arbitrary input — only return Ok/Err
            let _ = parse_plan(&input);
        }

        #[test]
        fn roundtrip_title(title in "[a-zA-Z0-9 ]{1,80}") {
            let md = format!("# {title}\n\n**Goal:** Some goal.\n");
            let meta = parse_plan(&md).unwrap();
            assert_eq!(meta.title, title.trim());
        }

        #[test]
        fn roundtrip_description(desc in "[a-zA-Z0-9 .!,]{1,100}") {
            let md = format!("# Title\n\n**Goal:** {desc}\n");
            let meta = parse_plan(&md).unwrap();
            assert_eq!(meta.description, desc.trim());
        }
    }

    #[test]
    fn parse_standard_plan() {
        let content = "\
# Automatic Task Status Hooks — Implementation Plan

> **For agentic workers:** ...

**Goal:** Automatically manage task status transitions via Claude Code hooks.

**Architecture:** Dispatch writes settings.json...
";
        let meta = parse_plan(content).unwrap();
        assert_eq!(meta.title, "Automatic Task Status Hooks");
        assert_eq!(
            meta.description,
            "Automatically manage task status transitions via Claude Code hooks."
        );
    }

    #[test]
    fn parse_title_without_suffix() {
        let content = "\
# Simple Feature

**Goal:** Do something simple.
";
        let meta = parse_plan(content).unwrap();
        assert_eq!(meta.title, "Simple Feature");
        assert_eq!(meta.description, "Do something simple.");
    }

    #[test]
    fn parse_missing_h1_is_error() {
        let content = "\
**Goal:** No heading here.
";
        let result = parse_plan(content);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("H1"), "Error should mention missing H1 heading");
    }

    #[test]
    fn parse_missing_goal_gives_empty_description() {
        let content = "\
# Feature Without Goal

**Architecture:** Some architecture.
";
        let meta = parse_plan(content).unwrap();
        assert_eq!(meta.title, "Feature Without Goal");
        assert_eq!(meta.description, "");
    }

    #[test]
    fn parse_h1_with_extra_whitespace() {
        let content = "\
#   Padded Title — Implementation Plan

**Goal:**   Spaced out goal.
";
        let meta = parse_plan(content).unwrap();
        assert_eq!(meta.title, "Padded Title");
        assert_eq!(meta.description, "Spaced out goal.");
    }
}
