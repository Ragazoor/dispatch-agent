#!/usr/bin/env bash
# Fetch open Dependabot vulnerability alerts for a list of GitHub repositories
# and output them as FeedItem JSON for use as a dispatch feed epic command.
#
# Requirements: gh (GitHub CLI, authenticated), jq
#
# Usage:
#   dispatch verify-feed scripts/fetch-security.sh   # validate output
#   # Or configure as feed_command on a feed epic with feed_interval_secs = 300
#
# Note: when used as a dispatch feed_command, use the absolute path to this
# script. Relative paths only work if dispatch is launched from the project root.
#
# To add/remove repos, edit the REPOS array below.

set -euo pipefail

# ---------------------------------------------------------------------------
# Repositories: edit repos.conf in the same directory (SSOT), or set REPOS
# directly below as a fallback when repos.conf is not present.
REPOS=()

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [[ -f "$SCRIPT_DIR/repos.conf" ]]; then
  # shellcheck source=repos.conf
  source "$SCRIPT_DIR/repos.conf"
fi
# ---------------------------------------------------------------------------

if [[ ${#REPOS[@]} -eq 0 ]]; then
  echo '[]'
  exit 0
fi

# NOTE: per_page=100 is the GitHub REST API maximum per request. Repos with
# more than 100 open Dependabot alerts will be silently truncated. This is
# acceptable for most projects; if you need full pagination use:
#   gh api --paginate "/repos/$repo/dependabot/alerts?state=open&per_page=100"
# (requires a different jq pipeline to flatten paginated responses).

for repo in "${REPOS[@]}"; do
  gh api "/repos/$repo/dependabot/alerts?state=open&per_page=100" 2>/dev/null \
    | jq --arg repo "$repo" '[.[] | {
        external_id: ("dependabot:\($repo)#" + (.number | tostring)),
        title: .security_advisory.summary,
        description: (.security_advisory.description // ""),
        url: .html_url,
        url_type: "security_alert",
        status: "backlog",
        tag: "fix",
        wrap_up_mode: "pr",
        labels: [($repo | split("/") | last)],
        sort_order: (
          {critical: 1, high: 2, medium: 3, low: 4}[
            .security_advisory.severity // "low"
          ] // 4
        )
      }]' \
    || echo '[]'
done | jq -s '[.[][]]'
