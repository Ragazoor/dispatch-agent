# Task: Built-in Fetch Commands

## Context

Third phase. Adds `dispatch fetch-reviews` and `dispatch fetch-security` CLI
subcommands that print `FeedItem` JSON to stdout. These are the feed commands stored
in the pre-seeded "Reviews" and "Security" epics.

Depends on phase 1 (FeedItem type) and can be developed alongside phase 2.

## Design

### CLI (src/main.rs)

Add two subcommands to the clap CLI:

```
dispatch fetch-reviews    # prints JSON to stdout, exits 0 on success
dispatch fetch-security   # prints JSON to stdout, exits 0 on success
```

Both subcommands:
1. Call the existing GitHub fetch functions from `src/github.rs`
2. Map results to `Vec<FeedItem>`
3. Print `serde_json::to_string(&items)` to stdout
4. Exit 0 on success, non-zero on error (stderr for error message)

### Mapping: ReviewPr → FeedItem

```
external_id:  "pr:{owner}/{repo}#{number}"
title:        "#{number} {pr.title}"  (truncated to reasonable length)
description:  "{pr.body}" (first 500 chars, or empty)
url:          pr.html_url
status:       ReviewWorkflowState → TaskStatus mapping:
                backlog        → "backlog"
                ongoing        → "running"
                action_required → "review"
                done           → "done"
```

For `fetch-reviews`, emit both reviewer PRs and Dependabot PRs in a single array.
Use `external_id` prefix to distinguish: `"pr:..."` for reviewer PRs,
`"dep:..."` for Dependabot.

### Mapping: SecurityAlert → FeedItem

```
external_id:  "{kind}:{owner}/{repo}#{number}"
              (kind = "dependabot" | "code-scanning")
title:        "[{SEVERITY}] {alert.description}"
description:  "{alert.url}"
url:          alert.html_url
status:       SecurityWorkflowState → TaskStatus (same mapping as above)
```

### Existing code to reuse

- `src/github.rs`: `fetch_review_prs()`, `fetch_bot_prs()`, `fetch_security_alerts()`
- `ReviewWorkflowState`, `SecurityWorkflowState` — read from DB before mapping status
  (so existing workflow state is reflected in the JSON output on subsequent runs, though
  the feed runner ignores status on re-runs anyway — this mainly matters for the initial
  seed)

### Config / env vars

The fetch commands inherit existing env var usage from `github.rs`:
- `GH_TOKEN` / GitHub CLI auth for API calls
- `DISPATCH_REPOS` or similar for repo list

No new config needed.

## TDD Checklist

- [ ] Write test: `fetch-reviews` with mocked `gh` output → valid `FeedItem` JSON
- [ ] Write test: `fetch-security` with mocked alert data → valid `FeedItem` JSON
- [ ] Write test: `external_id` is stable across two runs with same PR number
- [ ] Write test: command exits non-zero when GitHub fetch fails
- [ ] Write JSON schema validation test: output is parseable as `Vec<FeedItem>`
- Then implement minimum code to pass

## Files

- `src/main.rs` — add `fetch-reviews` and `fetch-security` subcommands
- `src/github.rs` — no changes needed; functions reused directly
- `src/feed.rs` — add `review_pr_to_feed_item()` and `alert_to_feed_item()` helpers
