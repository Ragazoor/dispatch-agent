//! Typed URL attached to a Task. Replaces the former heuristically-typed
//! `pr_url` String: the type is now stored explicitly rather than guessed.

use super::pr_number_from_url;
use serde::{Deserialize, Serialize};

/// The kind of URL attached to a task. Stored in the `url_type` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrlType {
    Pr,
    SecurityAlert,
    Issue,
    Other,
}

impl UrlType {
    pub const ALL: &'static [UrlType] = &[
        UrlType::Pr,
        UrlType::SecurityAlert,
        UrlType::Issue,
        UrlType::Other,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            UrlType::Pr => "pr",
            UrlType::SecurityAlert => "security_alert",
            UrlType::Issue => "issue",
            UrlType::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pr" => Some(UrlType::Pr),
            "security_alert" => Some(UrlType::SecurityAlert),
            "issue" => Some(UrlType::Issue),
            "other" => Some(UrlType::Other),
            _ => None,
        }
    }

    /// Best-effort classification of an arbitrary URL string, mirroring the
    /// SQL CASE used by the migration and feed ingest. Used where no explicit
    /// type is available (feed items today).
    pub fn infer(url: &str) -> Self {
        let clean = url.split(['?', '#']).next().unwrap_or(url);
        if clean.contains("/pull/") {
            UrlType::Pr
        } else if clean.contains("/issues/") {
            UrlType::Issue
        } else {
            UrlType::Other
        }
    }

    /// Human-readable type word (no number).
    pub fn type_word(&self) -> &'static str {
        match self {
            UrlType::Pr => "PR",
            UrlType::SecurityAlert => "Security Alert",
            UrlType::Issue => "Issue",
            UrlType::Other => "Link",
        }
    }
}

impl std::fmt::Display for UrlType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for UrlType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown url type: {s}"))
    }
}

impl rusqlite::types::FromSql for UrlType {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = String::column_result(value)?;
        UrlType::parse(&s).ok_or_else(|| {
            rusqlite::types::FromSqlError::Other(format!("bad url_type: {s}").into())
        })
    }
}

/// A typed URL attached to a task. `url` and `url_type` always travel together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskUrl {
    pub url: String,
    pub url_type: UrlType,
}

impl TaskUrl {
    pub fn new(url: impl Into<String>, url_type: UrlType) -> Self {
        Self {
            url: url.into(),
            url_type,
        }
    }

    pub fn is_pr(&self) -> bool {
        self.url_type == UrlType::Pr
    }

    /// Trailing numeric segment of the URL (PR/issue number), if any.
    pub fn pr_number(&self) -> Option<i64> {
        pr_number_from_url(&self.url)
    }

    /// Card/detail label: `"PR #123"`, `"Issue #7"`, `"Security Alert"`,
    /// `"Link"`. For Pr/Issue without a trailing number, the bare type word.
    pub fn label(&self) -> String {
        match self.url_type {
            UrlType::Pr | UrlType::Issue => match self.pr_number() {
                Some(n) => format!("{} #{n}", self.url_type.type_word()),
                None => self.url_type.type_word().to_string(),
            },
            UrlType::SecurityAlert | UrlType::Other => self.url_type.type_word().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// `UrlType::ALL` backs the update_task MCP schema's url_type enum
    /// (dispatch.rs) — a variant added there without updating `ALL` would
    /// silently under-advertise it.
    #[test]
    fn url_type_all_has_every_variant() {
        assert_eq!(UrlType::ALL.len(), 4);
    }

    #[test]
    fn url_type_roundtrip() {
        for ut in [
            UrlType::Pr,
            UrlType::SecurityAlert,
            UrlType::Issue,
            UrlType::Other,
        ] {
            assert_eq!(UrlType::parse(ut.as_str()), Some(ut));
        }
    }

    #[test]
    fn url_type_stored_strings() {
        assert_eq!(UrlType::Pr.as_str(), "pr");
        assert_eq!(UrlType::SecurityAlert.as_str(), "security_alert");
        assert_eq!(UrlType::Issue.as_str(), "issue");
        assert_eq!(UrlType::Other.as_str(), "other");
    }

    #[test]
    fn url_type_parse_unknown_is_none() {
        assert_eq!(UrlType::parse("nope"), None);
    }

    #[test]
    fn infer_classifies_pr_issue_other() {
        assert_eq!(
            UrlType::infer("https://github.com/o/r/pull/12"),
            UrlType::Pr
        );
        assert_eq!(
            UrlType::infer("https://github.com/o/r/issues/7"),
            UrlType::Issue
        );
        assert_eq!(
            UrlType::infer("https://example.com/whatever"),
            UrlType::Other
        );
    }

    #[test]
    fn label_pr_with_number() {
        let u = TaskUrl::new("https://github.com/o/r/pull/123", UrlType::Pr);
        assert_eq!(u.label(), "PR #123");
        assert_eq!(u.pr_number(), Some(123));
        assert!(u.is_pr());
    }

    #[test]
    fn label_issue_with_number() {
        let u = TaskUrl::new("https://github.com/o/r/issues/7", UrlType::Issue);
        assert_eq!(u.label(), "Issue #7");
        assert!(!u.is_pr());
    }

    #[test]
    fn label_pr_without_number_falls_back() {
        let u = TaskUrl::new("https://github.com/o/r/pull/", UrlType::Pr);
        assert_eq!(u.label(), "PR");
    }

    #[test]
    fn label_security_alert_and_other() {
        assert_eq!(
            TaskUrl::new(
                "https://github.com/o/r/security/dependabot/3",
                UrlType::SecurityAlert
            )
            .label(),
            "Security Alert"
        );
        assert_eq!(
            TaskUrl::new("https://example.com/x", UrlType::Other).label(),
            "Link"
        );
    }
}
