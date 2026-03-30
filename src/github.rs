use crate::models::{ReviewDecision, ReviewPr};
use crate::process::ProcessRunner;
use chrono::{DateTime, Utc};

/// Default authors to exclude (dependency bots).
pub const DEFAULT_EXCLUDED_AUTHORS: &[&str] = &[
    "dependabot[bot]",
    "scala-steward",
    "renovate[bot]",
];

/// Determine the effective review decision for a PR node.
///
/// Uses the overall `reviewDecision` for APPROVED and CHANGES_REQUESTED.
/// For REVIEW_REQUIRED, checks whether the viewer has left comments (plain PR
/// comments or COMMENTED-state reviews) and whether the PR author has responded
/// since (via a new comment or a new commit).
fn classify_review_decision(node: &serde_json::Value, viewer_login: &str) -> ReviewDecision {
    let decision_str = node["reviewDecision"].as_str().unwrap_or("REVIEW_REQUIRED");
    match decision_str {
        "APPROVED" => return ReviewDecision::Approved,
        "CHANGES_REQUESTED" => return ReviewDecision::ChangesRequested,
        _ => {}
    }

    let pr_author = node["author"]["login"].as_str().unwrap_or("");

    // Viewer's last plain comment
    let viewer_last_comment = node["comments"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["author"]["login"].as_str() == Some(viewer_login))
        .filter_map(|c| c["createdAt"].as_str()?.parse::<DateTime<Utc>>().ok())
        .max();

    // Viewer's last COMMENTED review
    let viewer_last_commented_review = node["reviews"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            r["author"]["login"].as_str() == Some(viewer_login)
                && r["state"].as_str() == Some("COMMENTED")
        })
        .filter_map(|r| r["submittedAt"].as_str()?.parse::<DateTime<Utc>>().ok())
        .max();

    let viewer_last_interaction = viewer_last_comment.max(viewer_last_commented_review);

    let Some(interaction_at) = viewer_last_interaction else {
        return ReviewDecision::ReviewRequired;
    };

    // Check if author has responded since the viewer's last interaction
    let author_last_comment = node["comments"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["author"]["login"].as_str() == Some(pr_author))
        .filter_map(|c| c["createdAt"].as_str()?.parse::<DateTime<Utc>>().ok())
        .max();

    let last_commit_date = node["commits"]["nodes"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|n| n["commit"]["committedDate"].as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok());

    let author_responded = author_last_comment.is_some_and(|t| t > interaction_at)
        || last_commit_date.is_some_and(|t| t > interaction_at);

    if author_responded {
        ReviewDecision::ReviewRequired
    } else {
        ReviewDecision::WaitingForResponse
    }
}

/// Parse the JSON response from `gh api graphql` into a list of ReviewPr.
fn parse_review_prs(json: &str) -> Result<Vec<ReviewPr>, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse JSON: {e}"))?;

    let viewer_login = root
        .pointer("/data/viewer/login")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let nodes = root
        .pointer("/data/search/nodes")
        .and_then(|v| v.as_array())
        .ok_or("Missing data.search.nodes in response")?;

    let mut prs = Vec::with_capacity(nodes.len());
    for node in nodes {
        // Safety net: skip drafts even if the query filter missed them
        if node["isDraft"].as_bool() == Some(true) {
            continue;
        }

        let review_decision = classify_review_decision(node, viewer_login);

        let labels = node["labels"]["nodes"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|l| l["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let created_at = node["createdAt"]
            .as_str()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or_else(Utc::now);
        let updated_at = node["updatedAt"]
            .as_str()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or_else(Utc::now);

        prs.push(ReviewPr {
            number: node["number"].as_i64().unwrap_or(0),
            title: node["title"].as_str().unwrap_or("").to_string(),
            author: node["author"]["login"].as_str().unwrap_or("").to_string(),
            repo: node["repository"]["nameWithOwner"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            url: node["url"].as_str().unwrap_or("").to_string(),
            is_draft: node["isDraft"].as_bool().unwrap_or(false),
            created_at,
            updated_at,
            additions: node["additions"].as_i64().unwrap_or(0),
            deletions: node["deletions"].as_i64().unwrap_or(0),
            review_decision,
            labels,
        });
    }

    Ok(prs)
}

/// Filter out PRs authored by excluded bot accounts.
fn filter_bots(prs: Vec<ReviewPr>, excluded: &[&str]) -> Vec<ReviewPr> {
    prs.into_iter()
        .filter(|pr| !excluded.iter().any(|bot| pr.author == *bot))
        .collect()
}

/// Fetch open PRs where the current user is a requested reviewer.
/// Uses `gh api graphql` via the provided ProcessRunner.
pub fn fetch_review_prs(
    runner: &dyn ProcessRunner,
    excluded_authors: &[&str],
) -> Result<Vec<ReviewPr>, String> {
    let query = r#"{
  viewer { login }
  search(query: "is:pr is:open review-requested:@me -is:draft", type: ISSUE, first: 100) {
    nodes {
      ... on PullRequest {
        number
        title
        url
        isDraft
        createdAt
        updatedAt
        additions
        deletions
        reviewDecision
        author { login }
        repository { nameWithOwner }
        labels(first: 10) { nodes { name } }
        comments(last: 50) { nodes { author { login } createdAt } }
        reviews(last: 20) { nodes { state author { login } submittedAt } }
        commits(last: 1) { nodes { commit { committedDate } } }
      }
    }
  }
}"#;

    let output = runner
        .run("gh", &["api", "graphql", "-f", &format!("query={query}")])
        .map_err(|e| format!("Failed to run gh: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh api graphql failed: {stderr}"));
    }

    let json = String::from_utf8_lossy(&output.stdout);
    let prs = parse_review_prs(&json)?;
    Ok(filter_bots(prs, excluded_authors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::MockProcessRunner;

    const SAMPLE_RESPONSE: &str = r#"{
        "data": {
            "viewer": {"login": "me"},
            "search": {
                "nodes": [
                    {
                        "number": 42,
                        "title": "Fix login flow",
                        "url": "https://github.com/acme/app/pull/42",
                        "isDraft": false,
                        "createdAt": "2026-03-28T10:00:00Z",
                        "updatedAt": "2026-03-29T14:00:00Z",
                        "additions": 15,
                        "deletions": 3,
                        "reviewDecision": "REVIEW_REQUIRED",
                        "author": {"login": "alice"},
                        "repository": {"nameWithOwner": "acme/app"},
                        "labels": {"nodes": [{"name": "bug"}, {"name": "urgent"}]},
                        "comments": {"nodes": []},
                        "reviews": {"nodes": []},
                        "commits": {"nodes": [{"commit": {"committedDate": "2026-03-28T10:00:00Z"}}]}
                    },
                    {
                        "number": 99,
                        "title": "Update sbt to 1.12",
                        "url": "https://github.com/acme/app/pull/99",
                        "isDraft": true,
                        "createdAt": "2026-03-27T08:00:00Z",
                        "updatedAt": "2026-03-27T08:00:00Z",
                        "additions": 1,
                        "deletions": 1,
                        "reviewDecision": "REVIEW_REQUIRED",
                        "author": {"login": "scala-steward"},
                        "repository": {"nameWithOwner": "acme/app"},
                        "labels": {"nodes": []},
                        "comments": {"nodes": []},
                        "reviews": {"nodes": []},
                        "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T08:00:00Z"}}]}
                    },
                    {
                        "number": 50,
                        "title": "Refactor auth module",
                        "url": "https://github.com/acme/backend/pull/50",
                        "isDraft": false,
                        "createdAt": "2026-03-25T12:00:00Z",
                        "updatedAt": "2026-03-29T09:00:00Z",
                        "additions": 200,
                        "deletions": 80,
                        "reviewDecision": "APPROVED",
                        "author": {"login": "bob"},
                        "repository": {"nameWithOwner": "acme/backend"},
                        "labels": {"nodes": [{"name": "refactor"}]},
                        "comments": {"nodes": []},
                        "reviews": {"nodes": []},
                        "commits": {"nodes": [{"commit": {"committedDate": "2026-03-25T12:00:00Z"}}]}
                    }
                ]
            }
        }
    }"#;

    #[test]
    fn parse_review_prs_extracts_all_fields() {
        let prs = parse_review_prs(SAMPLE_RESPONSE).unwrap();
        // Draft PR #99 is filtered out, leaving 2
        assert_eq!(prs.len(), 2);

        let pr = &prs[0];
        assert_eq!(pr.number, 42);
        assert_eq!(pr.title, "Fix login flow");
        assert_eq!(pr.author, "alice");
        assert_eq!(pr.repo, "acme/app");
        assert_eq!(pr.url, "https://github.com/acme/app/pull/42");
        assert!(!pr.is_draft);
        assert_eq!(pr.additions, 15);
        assert_eq!(pr.deletions, 3);
        assert_eq!(pr.review_decision, ReviewDecision::ReviewRequired);
        assert_eq!(pr.labels, vec!["bug", "urgent"]);
    }

    #[test]
    fn parse_review_prs_filters_drafts_and_handles_approved() {
        let prs = parse_review_prs(SAMPLE_RESPONSE).unwrap();
        // Draft PR #99 excluded; #50 (approved) is now index 1
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[1].review_decision, ReviewDecision::Approved);
    }

    #[test]
    fn parse_review_prs_empty_nodes() {
        let json = r#"{"data":{"search":{"nodes":[]}}}"#;
        let prs = parse_review_prs(json).unwrap();
        assert!(prs.is_empty());
    }

    #[test]
    fn parse_review_prs_invalid_json() {
        let result = parse_review_prs("not json");
        assert!(result.is_err());
    }

    #[test]
    fn parse_review_prs_null_review_decision_defaults_to_review_required() {
        let json = r#"{"data":{"viewer":{"login":"me"},"search":{"nodes":[{
            "number": 1, "title": "T", "url": "u", "isDraft": false,
            "createdAt": "2026-03-28T10:00:00Z", "updatedAt": "2026-03-28T10:00:00Z",
            "additions": 0, "deletions": 0,
            "reviewDecision": null,
            "author": {"login": "a"}, "repository": {"nameWithOwner": "o/r"},
            "labels": {"nodes": []},
            "comments": {"nodes": []},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }]}}}"#;
        let prs = parse_review_prs(json).unwrap();
        assert_eq!(prs[0].review_decision, ReviewDecision::ReviewRequired);
    }

    #[test]
    fn filter_bots_removes_excluded_authors() {
        let prs = parse_review_prs(SAMPLE_RESPONSE).unwrap();
        // Draft already filtered, leaves alice + bob; bot filter is N/A here
        let filtered = filter_bots(prs, DEFAULT_EXCLUDED_AUTHORS);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].author, "alice");
        assert_eq!(filtered[1].author, "bob");
    }

    #[test]
    fn filter_bots_empty_exclude_list_keeps_all() {
        let prs = parse_review_prs(SAMPLE_RESPONSE).unwrap();
        // Draft filtered out in parse, leaves 2
        let filtered = filter_bots(prs, &[]);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn fetch_review_prs_calls_gh_and_parses() {
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(SAMPLE_RESPONSE.as_bytes()),
        ]);
        let prs = fetch_review_prs(&runner, DEFAULT_EXCLUDED_AUTHORS).unwrap();
        assert_eq!(prs.len(), 2); // draft + bot filtered out

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "gh");
        assert!(calls[0].1.contains(&"graphql".to_string()));
    }

    #[test]
    fn fetch_review_prs_gh_failure() {
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::fail("gh: not authenticated"),
        ]);
        let result = fetch_review_prs(&runner, DEFAULT_EXCLUDED_AUTHORS);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not authenticated"));
    }

    #[test]
    fn fetch_review_prs_query_excludes_drafts() {
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(SAMPLE_RESPONSE.as_bytes()),
        ]);
        let _ = fetch_review_prs(&runner, DEFAULT_EXCLUDED_AUTHORS);
        let calls = runner.recorded_calls();
        let query_arg = calls[0].1.iter().find(|a| a.contains("query=")).unwrap();
        assert!(query_arg.contains("-is:draft"));
    }

    // -----------------------------------------------------------------------
    // classify_review_decision tests
    // -----------------------------------------------------------------------

    fn make_pr_node(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn classify_approved_takes_priority() {
        let node = make_pr_node(r#"{
            "reviewDecision": "APPROVED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }"#);
        assert_eq!(classify_review_decision(&node, "me"), ReviewDecision::Approved);
    }

    #[test]
    fn classify_changes_requested_takes_priority() {
        let node = make_pr_node(r#"{
            "reviewDecision": "CHANGES_REQUESTED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }"#);
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::ChangesRequested,
        );
    }

    #[test]
    fn classify_no_viewer_interaction() {
        let node = make_pr_node(r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": []},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }"#);
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::ReviewRequired,
        );
    }

    #[test]
    fn classify_viewer_comment_no_author_response() {
        let node = make_pr_node(r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T10:00:00Z"}}]}
        }"#);
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::WaitingForResponse,
        );
    }

    #[test]
    fn classify_viewer_commented_review_no_response() {
        let node = make_pr_node(r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": []},
            "reviews": {"nodes": [
                {"state": "COMMENTED", "author": {"login": "me"}, "submittedAt": "2026-03-28T12:00:00Z"}
            ]},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T10:00:00Z"}}]}
        }"#);
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::WaitingForResponse,
        );
    }

    #[test]
    fn classify_author_comment_after_viewer() {
        let node = make_pr_node(r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"},
                {"author": {"login": "alice"}, "createdAt": "2026-03-28T13:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T10:00:00Z"}}]}
        }"#);
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::ReviewRequired,
        );
    }

    #[test]
    fn classify_new_commit_after_viewer_comment() {
        let node = make_pr_node(r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-28T14:00:00Z"}}]}
        }"#);
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::ReviewRequired,
        );
    }

    #[test]
    fn classify_author_comment_before_viewer() {
        let node = make_pr_node(r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "alice"}, "createdAt": "2026-03-28T10:00:00Z"},
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T10:00:00Z"}}]}
        }"#);
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::WaitingForResponse,
        );
    }

    #[test]
    fn classify_draft_filtered_in_parse() {
        let json = r#"{"data":{"viewer":{"login":"me"},"search":{"nodes":[{
            "number": 1, "title": "T", "url": "u", "isDraft": true,
            "createdAt": "2026-03-28T10:00:00Z", "updatedAt": "2026-03-28T10:00:00Z",
            "additions": 0, "deletions": 0,
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "a"}, "repository": {"nameWithOwner": "o/r"},
            "labels": {"nodes": []},
            "comments": {"nodes": []},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }]}}}"#;
        let prs = parse_review_prs(json).unwrap();
        assert!(prs.is_empty());
    }
}
