use crate::models::{CiStatus, ReviewDecision, ReviewPr, Reviewer};
use crate::process::ProcessRunner;
use chrono::{DateTime, Utc};

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

    // Re-request is the strongest signal: if the viewer is in the current
    // reviewRequests list, the author explicitly asked for another look.
    let viewer_re_requested = node["reviewRequests"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|req| req["requestedReviewer"]["login"].as_str() == Some(viewer_login));

    if viewer_re_requested {
        return ReviewDecision::ReviewRequired;
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

    // Viewer's last review (any state: APPROVED, CHANGES_REQUESTED, COMMENTED)
    let viewer_last_review = node["reviews"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["author"]["login"].as_str() == Some(viewer_login))
        .filter_map(|r| r["submittedAt"].as_str()?.parse::<DateTime<Utc>>().ok())
        .max();

    let viewer_last_interaction = viewer_last_comment.max(viewer_last_review);

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

/// Extract reviewers from a PR node by merging completed reviews and pending
/// review requests. A reviewer who has left an APPROVED or CHANGES_REQUESTED
/// review gets that decision; a pending request (not yet reviewed) gets `None`.
fn parse_reviewers(node: &serde_json::Value) -> Vec<Reviewer> {
    let mut by_login: std::collections::HashMap<String, Option<ReviewDecision>> =
        std::collections::HashMap::new();

    // Completed reviews — latest state per reviewer
    if let Some(reviews) = node["reviews"]["nodes"].as_array() {
        for review in reviews {
            if let Some(login) = review["author"]["login"].as_str() {
                let decision = match review["state"].as_str() {
                    Some("APPROVED") => Some(ReviewDecision::Approved),
                    Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
                    _ => continue,
                };
                by_login.insert(login.to_string(), decision);
            }
        }
    }

    // Pending review requests — only add if not already reviewed
    if let Some(requests) = node["reviewRequests"]["nodes"].as_array() {
        for req in requests {
            if let Some(login) = req["requestedReviewer"]["login"].as_str() {
                by_login.entry(login.to_string()).or_insert(None);
            }
        }
    }

    by_login
        .into_iter()
        .map(|(login, decision)| Reviewer { login, decision })
        .collect()
}

/// Parse the JSON response from `gh api graphql` into a list of ReviewPr.
///
/// The response is expected to contain three aliased search results:
/// `data.requestedReview.nodes`, `data.alreadyReviewed.nodes`, and
/// `data.commented.nodes`. Nodes are deduplicated by URL so PRs appearing
/// in multiple lists are only included once.
fn parse_review_prs(json: &str) -> Result<Vec<ReviewPr>, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse JSON: {e}"))?;

    let viewer_login = root
        .pointer("/data/viewer/login")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Collect unique nodes from both aliased searches, deduplicating by URL.
    let mut seen_urls = std::collections::HashSet::new();
    let mut all_nodes: Vec<serde_json::Value> = Vec::new();
    for alias in &["requestedReview", "alreadyReviewed", "commented"] {
        let path = format!("/data/{alias}/nodes");
        if let Some(nodes) = root.pointer(&path).and_then(|v| v.as_array()) {
            for node in nodes {
                let url = node["url"].as_str().unwrap_or("").to_string();
                if !url.is_empty() && seen_urls.insert(url) {
                    all_nodes.push(node.clone());
                }
            }
        }
    }

    let mut prs = Vec::with_capacity(all_nodes.len());
    for node in &all_nodes {
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

        let body = node["body"].as_str().unwrap_or("").to_string();
        let head_ref = node["headRefName"].as_str().unwrap_or("").to_string();

        let ci_state = node["commits"]["nodes"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|n| n["commit"]["statusCheckRollup"]["state"].as_str());
        let ci_status = CiStatus::from_github(ci_state);

        let reviewers = parse_reviewers(node);

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
            body,
            head_ref,
            ci_status,
            reviewers,
        });
    }

    Ok(prs)
}

/// Parse the JSON response from `gh api graphql` into a list of the viewer's own PRs.
///
/// The response is expected to contain `data.viewer.login` and `data.myPrs.nodes`.
/// Drafts are filtered out.
fn parse_my_prs(json: &str) -> Result<Vec<ReviewPr>, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse JSON: {e}"))?;

    let viewer_login = root
        .pointer("/data/viewer/login")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let nodes = root
        .pointer("/data/myPrs/nodes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut prs = Vec::with_capacity(nodes.len());
    for node in &nodes {
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

        let body = node["body"].as_str().unwrap_or("").to_string();
        let head_ref = node["headRefName"].as_str().unwrap_or("").to_string();

        let ci_state = node["commits"]["nodes"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|n| n["commit"]["statusCheckRollup"]["state"].as_str());
        let ci_status = CiStatus::from_github(ci_state);

        let reviewers = parse_reviewers(node);

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
            body,
            head_ref,
            ci_status,
            reviewers,
        });
    }

    Ok(prs)
}

/// The PR fields fragment used in both search aliases.
const PR_FIELDS: &str = r#"... on PullRequest {
        number
        title
        url
        isDraft
        createdAt
        updatedAt
        additions
        deletions
        reviewDecision
        body
        headRefName
        author { login }
        repository { nameWithOwner }
        labels(first: 10) { nodes { name } }
        comments(last: 50) { nodes { author { login } createdAt } }
        reviews(last: 20) { nodes { state author { login } submittedAt } }
        reviewRequests(first: 10) { nodes { requestedReviewer { ... on User { login } } } }
        commits(last: 1) { nodes { commit { committedDate statusCheckRollup { state } } } }
      }"#;

/// Fetch open PRs where the current user is a requested or past reviewer.
///
/// Uses three aliased GraphQL searches in one request:
/// - `requestedReview`: PRs where `review-requested:@me` (pending review)
/// - `alreadyReviewed`: PRs where `reviewed-by:@me` (already reviewed, may need re-review)
/// - `commented`: PRs where `commenter:@me` (left comments but no formal review)
///
/// The three result sets are merged and deduplicated by URL client-side.
/// Own PRs (`-author:@me`), bot authors, and archived repos are excluded server-side.
pub fn fetch_review_prs(runner: &dyn ProcessRunner) -> Result<Vec<ReviewPr>, String> {
    let query = format!(
        r#"{{
  viewer {{ login }}
  requestedReview: search(query: "is:pr is:open review-requested:@me -is:draft -author:app/dependabot -author:app/renovate archived:false", type: ISSUE, first: 100) {{
    nodes {{
      {PR_FIELDS}
    }}
  }}
  alreadyReviewed: search(query: "is:pr is:open reviewed-by:@me -author:@me -is:draft -author:app/dependabot -author:app/renovate archived:false", type: ISSUE, first: 100) {{
    nodes {{
      {PR_FIELDS}
    }}
  }}
  commented: search(query: "is:pr is:open commenter:@me -author:@me -is:draft -author:app/dependabot -author:app/renovate archived:false", type: ISSUE, first: 100) {{
    nodes {{
      {PR_FIELDS}
    }}
  }}
}}"#
    );

    let output = runner
        .run("gh", &["api", "graphql", "-f", &format!("query={query}")])
        .map_err(|e| format!("Failed to run gh: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh api graphql failed: {stderr}"));
    }

    let json = String::from_utf8_lossy(&output.stdout);
    parse_review_prs(&json)
}

/// Fetch the current user's own open PRs (non-draft).
///
/// Uses `author:@me` in a single GraphQL search alias (`myPrs`).
/// Uses `gh api graphql` via the provided ProcessRunner.
pub fn fetch_my_prs(runner: &dyn ProcessRunner) -> Result<Vec<ReviewPr>, String> {
    let query = format!(
        r#"{{
  viewer {{ login }}
  myPrs: search(query: "is:pr is:open author:@me -is:draft archived:false", type: ISSUE, first: 100) {{
    nodes {{
      {PR_FIELDS}
    }}
  }}
}}"#
    );

    let output = runner
        .run("gh", &["api", "graphql", "-f", &format!("query={query}")])
        .map_err(|e| format!("Failed to run gh: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh api graphql failed: {stderr}"));
    }

    let json = String::from_utf8_lossy(&output.stdout);
    parse_my_prs(&json)
}

/// Fetch open dependency-bot PRs (dependabot + renovate), non-draft.
///
/// Uses two aliased GraphQL searches in one request:
/// - `dependabot`: PRs authored by `app/dependabot`
/// - `renovate`: PRs authored by `app/renovate`
///
/// The two result sets are merged and deduplicated by URL client-side.
pub fn fetch_bot_prs(runner: &dyn ProcessRunner) -> Result<Vec<ReviewPr>, String> {
    let query = format!(
        r#"{{
  viewer {{ login }}
  dependabot: search(query: "is:pr is:open author:app/dependabot -is:draft", type: ISSUE, first: 30) {{
    nodes {{
      {PR_FIELDS}
    }}
  }}
  renovate: search(query: "is:pr is:open author:app/renovate -is:draft", type: ISSUE, first: 30) {{
    nodes {{
      {PR_FIELDS}
    }}
  }}
}}"#
    );

    let output = runner
        .run("gh", &["api", "graphql", "-f", &format!("query={query}")])
        .map_err(|e| format!("Failed to run gh: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh api graphql failed: {stderr}"));
    }

    let json = String::from_utf8_lossy(&output.stdout);
    parse_bot_prs(&json)
}

/// Parse the bot PRs response (dependabot + renovate aliases), deduplicate by URL.
fn parse_bot_prs(json: &str) -> Result<Vec<ReviewPr>, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse JSON: {e}"))?;

    let viewer_login = root
        .pointer("/data/viewer/login")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut seen_urls = std::collections::HashSet::new();
    let mut all_nodes: Vec<serde_json::Value> = Vec::new();
    for alias in &["dependabot", "renovate"] {
        let path = format!("/data/{alias}/nodes");
        if let Some(nodes) = root.pointer(&path).and_then(|v| v.as_array()) {
            for node in nodes {
                let url = node["url"].as_str().unwrap_or("").to_string();
                if !url.is_empty() && seen_urls.insert(url) {
                    all_nodes.push(node.clone());
                }
            }
        }
    }

    let mut prs = Vec::with_capacity(all_nodes.len());
    for node in &all_nodes {
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

        let body = node["body"].as_str().unwrap_or("").to_string();
        let head_ref = node["headRefName"].as_str().unwrap_or("").to_string();

        let ci_state = node["commits"]["nodes"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|n| n["commit"]["statusCheckRollup"]["state"].as_str());
        let ci_status = CiStatus::from_github(ci_state);

        let reviewers = parse_reviewers(node);

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
            body,
            head_ref,
            ci_status,
            reviewers,
        });
    }

    prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(prs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::MockProcessRunner;

    // PR #42 is in requestedReview (pending review), PR #99 is a draft (filtered),
    // PR #50 is in alreadyReviewed (already reviewed, approved),
    // PR #60 is in commented (only left comments, no formal review).
    const SAMPLE_RESPONSE: &str = r#"{
        "data": {
            "viewer": {"login": "me"},
            "requestedReview": {
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
                    }
                ]
            },
            "alreadyReviewed": {
                "nodes": [
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
            },
            "commented": {
                "nodes": [
                    {
                        "number": 60,
                        "title": "Add logging to auth",
                        "url": "https://github.com/acme/backend/pull/60",
                        "isDraft": false,
                        "createdAt": "2026-03-26T08:00:00Z",
                        "updatedAt": "2026-03-29T10:00:00Z",
                        "additions": 30,
                        "deletions": 5,
                        "reviewDecision": "REVIEW_REQUIRED",
                        "author": {"login": "carol"},
                        "repository": {"nameWithOwner": "acme/backend"},
                        "labels": {"nodes": []},
                        "comments": {"nodes": [
                            {"author": {"login": "me"}, "createdAt": "2026-03-27T10:00:00Z"}
                        ]},
                        "reviews": {"nodes": []},
                        "commits": {"nodes": [{"commit": {"committedDate": "2026-03-26T08:00:00Z"}}]}
                    }
                ]
            }
        }
    }"#;

    #[test]
    fn parse_review_prs_extracts_all_fields() {
        let prs = parse_review_prs(SAMPLE_RESPONSE).unwrap();
        // Draft PR #99 is filtered out, leaving 3
        assert_eq!(prs.len(), 3);

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
        assert_eq!(prs.len(), 3);
        assert_eq!(prs[1].review_decision, ReviewDecision::Approved);
    }

    #[test]
    fn parse_review_prs_empty_nodes() {
        let json = r#"{"data":{"viewer":{"login":"me"},"requestedReview":{"nodes":[]},"alreadyReviewed":{"nodes":[]},"commented":{"nodes":[]}}}"#;
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
        let json = r#"{"data":{"viewer":{"login":"me"},"requestedReview":{"nodes":[{
            "number": 1, "title": "T", "url": "https://github.com/o/r/pull/1", "isDraft": false,
            "createdAt": "2026-03-28T10:00:00Z", "updatedAt": "2026-03-28T10:00:00Z",
            "additions": 0, "deletions": 0,
            "reviewDecision": null,
            "author": {"login": "a"}, "repository": {"nameWithOwner": "o/r"},
            "labels": {"nodes": []},
            "comments": {"nodes": []},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }]},"alreadyReviewed":{"nodes":[]},"commented":{"nodes":[]}}}"#;
        let prs = parse_review_prs(json).unwrap();
        assert_eq!(prs[0].review_decision, ReviewDecision::ReviewRequired);
    }

    #[test]
    fn fetch_review_prs_calls_gh_and_parses() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            SAMPLE_RESPONSE.as_bytes(),
        )]);
        let prs = fetch_review_prs(&runner).unwrap();
        assert_eq!(prs.len(), 3); // draft filtered out

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "gh");
        assert!(calls[0].1.contains(&"graphql".to_string()));
    }

    #[test]
    fn fetch_review_prs_gh_failure() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("gh: not authenticated")]);
        let result = fetch_review_prs(&runner);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not authenticated"));
    }

    #[test]
    fn fetch_review_prs_query_includes_all_searches() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            SAMPLE_RESPONSE.as_bytes(),
        )]);
        let _ = fetch_review_prs(&runner);
        let calls = runner.recorded_calls();
        let query_arg = calls[0].1.iter().find(|a| a.contains("query=")).unwrap();
        assert!(
            query_arg.contains("review-requested:@me"),
            "missing review-requested qualifier"
        );
        assert!(
            query_arg.contains("reviewed-by:@me"),
            "missing reviewed-by qualifier"
        );
        assert!(
            query_arg.contains("commenter:@me"),
            "missing commenter qualifier"
        );
        assert!(query_arg.contains("-is:draft"));
        assert!(query_arg.contains("-author:app/dependabot"));
        assert!(query_arg.contains("-author:app/renovate"));
        assert!(query_arg.contains("-author:@me"));
    }

    #[test]
    fn parse_review_prs_deduplicates_across_aliases() {
        // PR #42 appears in all three aliases — should only be counted once.
        let json = r#"{
            "data": {
                "viewer": {"login": "me"},
                "requestedReview": {"nodes": [{
                    "number": 42, "title": "Fix login flow",
                    "url": "https://github.com/acme/app/pull/42",
                    "isDraft": false,
                    "createdAt": "2026-03-28T10:00:00Z", "updatedAt": "2026-03-28T10:00:00Z",
                    "additions": 15, "deletions": 3,
                    "reviewDecision": "REVIEW_REQUIRED",
                    "author": {"login": "alice"}, "repository": {"nameWithOwner": "acme/app"},
                    "labels": {"nodes": []}, "comments": {"nodes": []},
                    "reviews": {"nodes": []}, "commits": {"nodes": []}
                }]},
                "alreadyReviewed": {"nodes": [{
                    "number": 42, "title": "Fix login flow",
                    "url": "https://github.com/acme/app/pull/42",
                    "isDraft": false,
                    "createdAt": "2026-03-28T10:00:00Z", "updatedAt": "2026-03-28T10:00:00Z",
                    "additions": 15, "deletions": 3,
                    "reviewDecision": "REVIEW_REQUIRED",
                    "author": {"login": "alice"}, "repository": {"nameWithOwner": "acme/app"},
                    "labels": {"nodes": []}, "comments": {"nodes": []},
                    "reviews": {"nodes": []}, "commits": {"nodes": []}
                }]},
                "commented": {"nodes": [{
                    "number": 42, "title": "Fix login flow",
                    "url": "https://github.com/acme/app/pull/42",
                    "isDraft": false,
                    "createdAt": "2026-03-28T10:00:00Z", "updatedAt": "2026-03-28T10:00:00Z",
                    "additions": 15, "deletions": 3,
                    "reviewDecision": "REVIEW_REQUIRED",
                    "author": {"login": "alice"}, "repository": {"nameWithOwner": "acme/app"},
                    "labels": {"nodes": []}, "comments": {"nodes": []},
                    "reviews": {"nodes": []}, "commits": {"nodes": []}
                }]}
            }
        }"#;
        let prs = parse_review_prs(json).unwrap();
        assert_eq!(prs.len(), 1, "duplicate should be deduplicated");
        assert_eq!(prs[0].number, 42);
    }

    // -----------------------------------------------------------------------
    // classify_review_decision tests
    // -----------------------------------------------------------------------

    fn make_pr_node(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn classify_approved_takes_priority() {
        let node = make_pr_node(
            r#"{
            "reviewDecision": "APPROVED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }"#,
        );
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::Approved
        );
    }

    #[test]
    fn classify_changes_requested_takes_priority() {
        let node = make_pr_node(
            r#"{
            "reviewDecision": "CHANGES_REQUESTED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }"#,
        );
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::ChangesRequested,
        );
    }

    #[test]
    fn classify_no_viewer_interaction() {
        let node = make_pr_node(
            r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": []},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }"#,
        );
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::ReviewRequired,
        );
    }

    #[test]
    fn classify_viewer_comment_no_author_response() {
        let node = make_pr_node(
            r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T10:00:00Z"}}]}
        }"#,
        );
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::WaitingForResponse,
        );
    }

    #[test]
    fn classify_viewer_commented_review_no_response() {
        let node = make_pr_node(
            r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": []},
            "reviews": {"nodes": [
                {"state": "COMMENTED", "author": {"login": "me"}, "submittedAt": "2026-03-28T12:00:00Z"}
            ]},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T10:00:00Z"}}]}
        }"#,
        );
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::WaitingForResponse,
        );
    }

    #[test]
    fn classify_viewer_approved_review_no_response() {
        // Viewer approved but overall PR still needs other reviews.
        let node = make_pr_node(
            r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": []},
            "reviews": {"nodes": [
                {"state": "APPROVED", "author": {"login": "me"}, "submittedAt": "2026-03-28T12:00:00Z"}
            ]},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T10:00:00Z"}}]}
        }"#,
        );
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::WaitingForResponse,
        );
    }

    #[test]
    fn classify_author_comment_after_viewer() {
        let node = make_pr_node(
            r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"},
                {"author": {"login": "alice"}, "createdAt": "2026-03-28T13:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T10:00:00Z"}}]}
        }"#,
        );
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::ReviewRequired,
        );
    }

    #[test]
    fn classify_new_commit_after_viewer_comment() {
        let node = make_pr_node(
            r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-28T14:00:00Z"}}]}
        }"#,
        );
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::ReviewRequired,
        );
    }

    #[test]
    fn classify_author_comment_before_viewer() {
        let node = make_pr_node(
            r#"{
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "comments": {"nodes": [
                {"author": {"login": "alice"}, "createdAt": "2026-03-28T10:00:00Z"},
                {"author": {"login": "me"}, "createdAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviews": {"nodes": []},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-27T10:00:00Z"}}]}
        }"#,
        );
        assert_eq!(
            classify_review_decision(&node, "me"),
            ReviewDecision::WaitingForResponse,
        );
    }

    #[test]
    fn parse_review_prs_extracts_ci_status_and_body() {
        let json = r#"{"data":{"viewer":{"login":"me"},"requestedReview":{"nodes":[{
            "number": 77,
            "title": "Fix auth bug",
            "url": "https://github.com/acme/app/pull/77",
            "isDraft": false,
            "createdAt": "2026-03-28T10:00:00Z",
            "updatedAt": "2026-03-29T14:00:00Z",
            "additions": 10,
            "deletions": 2,
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "alice"},
            "repository": {"nameWithOwner": "acme/app"},
            "labels": {"nodes": []},
            "comments": {"nodes": []},
            "reviews": {"nodes": []},
            "body": "This fixes the auth bug",
            "headRefName": "fix-auth-bug",
            "commits": {"nodes": [{"commit": {"committedDate": "2026-03-28T10:00:00Z", "statusCheckRollup": {"state": "SUCCESS"}}}]},
            "reviewRequests": {"nodes": []}
        }]},"alreadyReviewed":{"nodes":[]},"commented":{"nodes":[]}}}"#;
        let prs = parse_review_prs(json).unwrap();
        assert_eq!(prs.len(), 1);
        let pr = &prs[0];
        assert_eq!(pr.ci_status, CiStatus::Success);
        assert_eq!(pr.body, "This fixes the auth bug");
        assert_eq!(pr.head_ref, "fix-auth-bug");
    }

    #[test]
    fn parse_reviewers_from_reviews_and_requests() {
        let node = make_pr_node(
            r#"{
            "reviews": {"nodes": [
                {"state": "APPROVED", "author": {"login": "bob"}, "submittedAt": "2026-03-28T12:00:00Z"}
            ]},
            "reviewRequests": {"nodes": [
                {"requestedReviewer": {"login": "carol"}}
            ]}
        }"#,
        );
        let mut reviewers = parse_reviewers(&node);
        reviewers.sort_by(|a, b| a.login.cmp(&b.login));
        assert_eq!(reviewers.len(), 2);
        assert_eq!(reviewers[0].login, "bob");
        assert_eq!(reviewers[0].decision, Some(ReviewDecision::Approved));
        assert_eq!(reviewers[1].login, "carol");
        assert_eq!(reviewers[1].decision, None);
    }

    #[test]
    fn classify_rerequest_moves_to_review_required() {
        let node = serde_json::json!({
            "reviewDecision": "REVIEW_REQUIRED",
            "author": { "login": "alice" },
            "comments": { "nodes": [
                { "author": { "login": "viewer" }, "createdAt": "2026-01-01T01:00:00Z" }
            ] },
            "reviews": { "nodes": [] },
            "commits": { "nodes": [{ "commit": { "committedDate": "2026-01-01T00:00:00Z" } }] },
            "reviewRequests": { "nodes": [
                { "requestedReviewer": { "login": "viewer" } }
            ] }
        });
        let decision = classify_review_decision(&node, "viewer");
        assert_eq!(decision, ReviewDecision::ReviewRequired);
    }

    #[test]
    fn classify_no_rerequest_stays_waiting() {
        let node = serde_json::json!({
            "reviewDecision": "REVIEW_REQUIRED",
            "author": { "login": "alice" },
            "comments": { "nodes": [
                { "author": { "login": "viewer" }, "createdAt": "2026-01-01T01:00:00Z" }
            ] },
            "reviews": { "nodes": [] },
            "commits": { "nodes": [{ "commit": { "committedDate": "2026-01-01T00:00:00Z" } }] },
            "reviewRequests": { "nodes": [] }
        });
        let decision = classify_review_decision(&node, "viewer");
        assert_eq!(decision, ReviewDecision::WaitingForResponse);
    }

    #[test]
    fn classify_draft_filtered_in_parse() {
        let json = r#"{"data":{"viewer":{"login":"me"},"requestedReview":{"nodes":[{
            "number": 1, "title": "T", "url": "https://github.com/o/r/pull/1", "isDraft": true,
            "createdAt": "2026-03-28T10:00:00Z", "updatedAt": "2026-03-28T10:00:00Z",
            "additions": 0, "deletions": 0,
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "a"}, "repository": {"nameWithOwner": "o/r"},
            "labels": {"nodes": []},
            "comments": {"nodes": []},
            "reviews": {"nodes": []},
            "commits": {"nodes": []}
        }]},"alreadyReviewed":{"nodes":[]},"commented":{"nodes":[]}}}"#;
        let prs = parse_review_prs(json).unwrap();
        assert!(prs.is_empty());
    }

    /// Integration test: calls the real `gh` CLI to verify fetch works end-to-end.
    /// Run with: cargo test fetch_review_prs_real -- --ignored
    #[test]
    #[ignore]
    fn fetch_review_prs_real() {
        let runner = crate::process::RealProcessRunner;
        let result = fetch_review_prs(&runner);
        eprintln!("result: {result:?}");
        assert!(result.is_ok(), "fetch failed: {}", result.unwrap_err());
        let prs = result.unwrap();
        eprintln!("fetched {} PRs", prs.len());
        for pr in &prs {
            eprintln!("  #{} {} [{:?}]", pr.number, pr.title, pr.review_decision);
        }
    }

    // -----------------------------------------------------------------------
    // parse_my_prs / fetch_my_prs tests
    // -----------------------------------------------------------------------

    const MY_PRS_RESPONSE: &str = r#"{
    "data": {
        "viewer": {"login": "me"},
        "myPrs": {
            "nodes": [
                {
                    "number": 101,
                    "title": "My feature PR",
                    "url": "https://github.com/acme/app/pull/101",
                    "isDraft": false,
                    "createdAt": "2026-03-28T10:00:00Z",
                    "updatedAt": "2026-03-29T14:00:00Z",
                    "additions": 50,
                    "deletions": 10,
                    "reviewDecision": "REVIEW_REQUIRED",
                    "author": {"login": "me"},
                    "repository": {"nameWithOwner": "acme/app"},
                    "labels": {"nodes": [{"name": "feature"}]},
                    "comments": {"nodes": []},
                    "reviews": {"nodes": []},
                    "body": "Adds a new feature",
                    "headRefName": "my-feature",
                    "commits": {"nodes": [{"commit": {"committedDate": "2026-03-28T10:00:00Z", "statusCheckRollup": {"state": "SUCCESS"}}}]},
                    "reviewRequests": {"nodes": [{"requestedReviewer": {"login": "alice"}}]}
                }
            ]
        }
    }
}"#;

    #[test]
    fn parse_my_prs_extracts_fields() {
        let prs = parse_my_prs(MY_PRS_RESPONSE).unwrap();
        assert_eq!(prs.len(), 1);
        let pr = &prs[0];
        assert_eq!(pr.number, 101);
        assert_eq!(pr.title, "My feature PR");
        assert_eq!(pr.author, "me");
        assert_eq!(pr.review_decision, ReviewDecision::ReviewRequired);
        assert_eq!(pr.ci_status, CiStatus::Success);
        assert_eq!(pr.reviewers.len(), 1);
        assert_eq!(pr.reviewers[0].login, "alice");
    }

    #[test]
    fn parse_my_prs_filters_drafts() {
        let json = r#"{"data":{"viewer":{"login":"me"},"myPrs":{"nodes":[{
            "number": 1, "title": "T", "url": "https://github.com/o/r/pull/1", "isDraft": true,
            "createdAt": "2026-03-28T10:00:00Z", "updatedAt": "2026-03-28T10:00:00Z",
            "additions": 0, "deletions": 0,
            "reviewDecision": "REVIEW_REQUIRED",
            "author": {"login": "me"}, "repository": {"nameWithOwner": "o/r"},
            "labels": {"nodes": []}, "comments": {"nodes": []},
            "reviews": {"nodes": []}, "body": "", "headRefName": "x",
            "commits": {"nodes": []}, "reviewRequests": {"nodes": []}
        }]}}}"#;
        let prs = parse_my_prs(json).unwrap();
        assert!(prs.is_empty());
    }

    #[test]
    fn fetch_my_prs_calls_gh_with_author_query() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            MY_PRS_RESPONSE.as_bytes(),
        )]);
        let prs = fetch_my_prs(&runner).unwrap();
        assert_eq!(prs.len(), 1);

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 1);
        let query_arg = calls[0].1.iter().find(|a| a.contains("query=")).unwrap();
        assert!(
            query_arg.contains("author:@me"),
            "missing author:@me qualifier"
        );
        assert!(query_arg.contains("-is:draft"));
    }
}
