use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::TaskId;
use crate::define_id_newtype;

define_id_newtype!(LearningId, learning_id_tests);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningKind {
    Pitfall,
    Convention,
    Preference,
    ToolRecommendation,
    Procedural,
    Landscape,
}

impl LearningKind {
    pub const ALL: &'static [LearningKind] = &[
        LearningKind::Pitfall,
        LearningKind::Convention,
        LearningKind::Preference,
        LearningKind::ToolRecommendation,
        LearningKind::Procedural,
        LearningKind::Landscape,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LearningKind::Pitfall => "pitfall",
            LearningKind::Convention => "convention",
            LearningKind::Preference => "preference",
            LearningKind::ToolRecommendation => "tool_recommendation",
            LearningKind::Procedural => "procedural",
            LearningKind::Landscape => "landscape",
        }
    }

    pub fn display_label(self) -> &'static str {
        match self {
            LearningKind::Pitfall => "Pitfall",
            LearningKind::Convention => "Convention",
            LearningKind::Preference => "Preference",
            LearningKind::ToolRecommendation => "Tool recommendation",
            LearningKind::Procedural => "Procedural",
            LearningKind::Landscape => "Landscape",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pitfall" => Some(LearningKind::Pitfall),
            "convention" => Some(LearningKind::Convention),
            "preference" => Some(LearningKind::Preference),
            "tool_recommendation" => Some(LearningKind::ToolRecommendation),
            "procedural" => Some(LearningKind::Procedural),
            "landscape" => Some(LearningKind::Landscape),
            _ => None,
        }
    }
}

impl std::fmt::Display for LearningKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for LearningKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown learning kind: {s}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningScope {
    User,
    Repo,
    Epic,
    Task,
}

impl LearningScope {
    pub const ALL: &'static [LearningScope] = &[
        LearningScope::User,
        LearningScope::Repo,
        LearningScope::Epic,
        LearningScope::Task,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LearningScope::User => "user",
            LearningScope::Repo => "repo",
            LearningScope::Epic => "epic",
            LearningScope::Task => "task",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(LearningScope::User),
            "repo" => Some(LearningScope::Repo),
            "epic" => Some(LearningScope::Epic),
            "task" => Some(LearningScope::Task),
            _ => None,
        }
    }
}

impl std::fmt::Display for LearningScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for LearningScope {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown learning scope: {s}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningStatus {
    Approved,
    Rejected,
    Archived,
}

impl LearningStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            LearningStatus::Approved => "approved",
            LearningStatus::Rejected => "rejected",
            LearningStatus::Archived => "archived",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "approved" => Ok(LearningStatus::Approved),
            "rejected" => Ok(LearningStatus::Rejected),
            "archived" => Ok(LearningStatus::Archived),
            other => Err(format!("unknown learning status: {other}")),
        }
    }
}

impl std::fmt::Display for LearningStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for LearningStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Debug, Clone)]
pub struct Learning {
    pub id: LearningId,
    pub kind: LearningKind,
    pub summary: String,
    pub detail: Option<String>,
    pub scope: LearningScope,
    pub scope_ref: Option<String>,
    pub tags: Vec<String>,
    pub status: LearningStatus,
    pub source_task_id: Option<TaskId>,
    pub upvote_count: i64,
    pub last_upvoted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningVerdict {
    Helped,
    Wrong,
}

impl LearningVerdict {
    pub const ALL: &'static [LearningVerdict] = &[LearningVerdict::Helped, LearningVerdict::Wrong];

    pub fn as_str(self) -> &'static str {
        match self {
            LearningVerdict::Helped => "helped",
            LearningVerdict::Wrong => "wrong",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "helped" => Ok(LearningVerdict::Helped),
            "wrong" => Ok(LearningVerdict::Wrong),
            other => Err(format!("unknown verdict: {other}")),
        }
    }
}

impl std::fmt::Display for LearningVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for LearningVerdict {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalSource {
    PromptInjection,
    /// Legacy — no longer produced; kept for existing DB records.
    Procedural,
    QueryLearnings,
}

impl RetrievalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            RetrievalSource::PromptInjection => "prompt_injection",
            RetrievalSource::Procedural => "procedural",
            RetrievalSource::QueryLearnings => "query_learnings",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "prompt_injection" => Ok(RetrievalSource::PromptInjection),
            "procedural" => Ok(RetrievalSource::Procedural),
            "query_learnings" => Ok(RetrievalSource::QueryLearnings),
            other => Err(format!("unknown retrieval source: {other}")),
        }
    }
}

impl std::fmt::Display for RetrievalSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for RetrievalSource {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Debug, Clone)]
pub struct LearningRetrieval {
    pub id: i64,
    pub task_id: TaskId,
    pub learning_id: LearningId,
    pub source: RetrievalSource,
    pub retrieved_at: DateTime<Utc>,
}

#[cfg(test)]
mod validation_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn needs_review_no_longer_parses() {
        // The needs_review status was removed; parsing it must hard-error per
        // the storage-boundary policy (core.allium).
        assert!(LearningStatus::parse("needs_review").is_err());
    }

    #[test]
    fn verdict_round_trips() {
        assert_eq!(
            LearningVerdict::parse("helped").unwrap(),
            LearningVerdict::Helped
        );
        assert_eq!(
            LearningVerdict::parse("wrong").unwrap(),
            LearningVerdict::Wrong
        );
        // `unused` was removed — it is now an unknown verdict.
        assert!(LearningVerdict::parse("unused").is_err());
        assert!(LearningVerdict::parse("bogus").is_err());
        assert_eq!(LearningVerdict::Helped.as_str(), "helped");
        assert_eq!(LearningVerdict::Wrong.as_str(), "wrong");
    }

    #[test]
    fn retrieval_source_round_trips() {
        assert_eq!(
            RetrievalSource::parse("prompt_injection").unwrap(),
            RetrievalSource::PromptInjection
        );
        assert_eq!(
            RetrievalSource::parse("procedural").unwrap(),
            RetrievalSource::Procedural
        );
        assert_eq!(
            RetrievalSource::parse("query_learnings").unwrap(),
            RetrievalSource::QueryLearnings
        );
        assert!(RetrievalSource::parse("nope").is_err());
        assert_eq!(
            RetrievalSource::PromptInjection.as_str(),
            "prompt_injection"
        );
    }
}
