// ---------------------------------------------------------------------------
// ReviewDecision — review status for a PR
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    ReviewRequired,
    WaitingForResponse,
    ChangesRequested,
    Approved,
}

impl ReviewDecision {
    pub const ALL: [Self; 4] = [
        Self::ReviewRequired,
        Self::WaitingForResponse,
        Self::ChangesRequested,
        Self::Approved,
    ];

    pub const COLUMN_COUNT: usize = Self::ALL.len();

    pub fn column_index(self) -> usize {
        match self {
            Self::ReviewRequired => 0,
            Self::WaitingForResponse => 1,
            Self::ChangesRequested => 2,
            Self::Approved => 3,
        }
    }

    pub fn from_column_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Self::ReviewRequired),
            1 => Some(Self::WaitingForResponse),
            2 => Some(Self::ChangesRequested),
            3 => Some(Self::Approved),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReviewRequired => "Needs Review",
            Self::WaitingForResponse => "Waiting for Response",
            Self::ChangesRequested => "Changes Requested",
            Self::Approved => "Approved",
        }
    }

    /// Parse from GitHub GraphQL `reviewDecision` field value.
    /// Note: `WaitingForResponse` has no wire value — it is derived client-side.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "REVIEW_REQUIRED" => Some(Self::ReviewRequired),
            "CHANGES_REQUESTED" => Some(Self::ChangesRequested),
            "APPROVED" => Some(Self::Approved),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// PR URL helpers
// ---------------------------------------------------------------------------

pub fn pr_number_from_url(url: &str) -> Option<i64> {
    url.split(['?', '#'])
        .next()
        .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
        .and_then(|s| s.parse::<i64>().ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn review_decision_as_str_waiting_for_response() {
        assert_eq!(
            ReviewDecision::WaitingForResponse.as_str(),
            "Waiting for Response"
        );
    }

    #[test]
    fn review_decision_parse_rejects_unknown_and_derived_values() {
        assert_eq!(ReviewDecision::parse(""), None);
        assert_eq!(ReviewDecision::parse("bogus"), None);
        // GitHub values are SCREAMING_SNAKE_CASE; mixed case must not match
        assert_eq!(ReviewDecision::parse("Review_Required"), None);
        // WaitingForResponse is derived client-side; the GitHub API never emits this
        assert_eq!(ReviewDecision::parse("WAITING_FOR_RESPONSE"), None);
    }

    // --- ReviewDecision columns ---

    #[test]
    fn review_decision_column_count() {
        assert_eq!(ReviewDecision::COLUMN_COUNT, 4);
        assert_eq!(ReviewDecision::ALL.len(), 4);
    }

    #[test]
    fn review_decision_column_index_roundtrip() {
        for (i, decision) in ReviewDecision::ALL.iter().enumerate() {
            assert_eq!(decision.column_index(), i);
        }
    }

    #[test]
    fn review_decision_from_column_index() {
        assert_eq!(
            ReviewDecision::from_column_index(0),
            Some(ReviewDecision::ReviewRequired)
        );
        assert_eq!(
            ReviewDecision::from_column_index(1),
            Some(ReviewDecision::WaitingForResponse)
        );
        assert_eq!(
            ReviewDecision::from_column_index(2),
            Some(ReviewDecision::ChangesRequested)
        );
        assert_eq!(
            ReviewDecision::from_column_index(3),
            Some(ReviewDecision::Approved)
        );
        assert_eq!(ReviewDecision::from_column_index(4), None);
    }

    #[test]
    fn review_decision_as_str() {
        assert_eq!(ReviewDecision::ReviewRequired.as_str(), "Needs Review");
        assert_eq!(
            ReviewDecision::ChangesRequested.as_str(),
            "Changes Requested"
        );
        assert_eq!(ReviewDecision::Approved.as_str(), "Approved");
    }

    #[test]
    fn review_decision_parse() {
        assert_eq!(
            ReviewDecision::parse("REVIEW_REQUIRED"),
            Some(ReviewDecision::ReviewRequired)
        );
        assert_eq!(
            ReviewDecision::parse("CHANGES_REQUESTED"),
            Some(ReviewDecision::ChangesRequested)
        );
        assert_eq!(
            ReviewDecision::parse("APPROVED"),
            Some(ReviewDecision::Approved)
        );
        assert_eq!(ReviewDecision::parse("bogus"), None);
        assert_eq!(ReviewDecision::parse(""), None);
    }

    // --- pr_number_from_url ---

    #[test]
    fn pr_number_from_standard_url() {
        assert_eq!(
            pr_number_from_url("https://github.com/org/repo/pull/42"),
            Some(42)
        );
    }

    #[test]
    fn pr_number_from_url_with_trailing_slash() {
        assert_eq!(
            pr_number_from_url("https://github.com/org/repo/pull/42/"),
            Some(42)
        );
    }

    #[test]
    fn pr_number_from_url_with_query_params() {
        assert_eq!(
            pr_number_from_url("https://github.com/org/repo/pull/42?diff=split"),
            Some(42)
        );
    }

    #[test]
    fn pr_number_from_url_no_number() {
        assert_eq!(pr_number_from_url("https://github.com/org/repo"), None);
    }

    #[test]
    fn pr_number_from_empty_url() {
        assert_eq!(pr_number_from_url(""), None);
    }

    #[test]
    fn pr_number_from_url_with_fragment() {
        assert_eq!(
            pr_number_from_url("https://github.com/org/repo/pull/42#issuecomment-123"),
            Some(42)
        );
    }
}
