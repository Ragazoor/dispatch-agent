#!/usr/bin/env bash
# fetch-reviews.sh — outputs every open PR you are involved with as a single,
# deduped FeedItem JSON array, for use as a dispatch feed_command.
#
# Prerequisites: gh CLI (https://cli.github.com/) and jq must be in PATH.
#
# Setup:
#   1. Copy this file to scripts/local/fetch-reviews.sh
#   2. Edit repos.conf in the same directory (the REPOS array) to list the
#      "owner/repo" slugs you want review-related PR activity for. This is
#      the same SSOT fetch-cve.sh reads, so reviews and CVEs stay scoped to
#      one repo list. Feeds My/Team/Bots exactly as before.
#   3. Optionally edit org.conf in the same directory (the ORGS array) to
#      list GitHub org slugs you want review activity for. This is a
#      SEPARATE scope: it re-runs three of the four review-related queries
#      (excluding the team-inclusive review-requested:@me — see below)
#      against whole orgs instead of the repo list, and every match routes
#      to My Reviews only (it never widens Team Reviews or Bots).
#   4. Point the parent "Reviews" epic's feed_command at the local copy.
#      There is NO scope argument — the dispatch role router (feed_role =
#      reviews_parent) splits the single emission into My / Team / Bots
#      sub-epics using the per-PR `signals` this script attaches.
#
# What it emits:
#   ONE FeedItem array covering the union of these open-PR searches, each PR
#   tagged with the signal(s) that matched it:
#     - review-requested:@me        -> signal "team-request" (direct + team)
#     - user-review-requested:@me   -> signal "direct-request" (direct only)
#     - reviewed-by:@me             -> signal "reviewed"
#     - commenter:@me -author:@me   -> signal "commented" (excludes your own PRs)
#   These four are scoped by repos.conf's REPOS list. THREE of the four are
#   run again, scoped by org.conf's ORGS list instead, and every match from
#   that pass carries a single shared signal so it always lands in My
#   Reviews:
#     - user-review-requested:@me | reviewed-by:@me |
#       commenter:@me -author:@me   (per org)  -> signal "org-review"
#   review-requested:@me is deliberately excluded from the org-scoped pass:
#   it also matches PRs requested from a team you belong to (not just you
#   personally), which org-wide would sweep in team-request noise.
#
#   A fifth pass covers bot-authored PRs you have NO review involvement with,
#   which the four review queries above can never reach:
#     - author:<bot>              (per bot in bots.conf, per repo in
#                                  repos.conf) -> signal "author-bot"
#   Bot logins go in bots.conf (the BOT_AUTHORS array) because they are
#   deployment-specific: a self-hosted Renovate app is named after the org
#   that installed it (e.g. app/kognic-renovate, not app/renovate). Absent or
#   empty BOT_AUTHORS skips the pass. It emits "author-bot" rather than a new
#   signal because dispatch's router already maps {author-bot} to Bots, and a
#   bot PR you HAVE reviewed still merges to one item and stays in My Reviews
#   (engagement wins over the bot rule). Repo-scoped only — org-wide bot
#   authorship would sweep in every dependency PR in the org.
#
#   Plus per-PR author signals: "author-bot" when the author login ends in
#   "[bot]" (Renovate/Dependabot), "author-me" when the author is the gh user.
#
#   A FINAL pass, after dedup, attaches each PR's CI status as a label:
#     - "ci:pass"    the head commit's check rollup succeeded
#     - "ci:fail"    the rollup failed or errored
#     - "ci:pending" the rollup is running, or expected but not started
#   A PR with NO checks gets NO ci label: absence means "nothing ran", which is
#   distinct from all three and must not be collapsed into "pass". `gh search
#   prs --json` exposes no check field at all, so this is a separate request —
#   ONE batched `gh api graphql` per poll over every deduped PR, not one call
#   per PR. A failed fetch degrades to no ci label, never a wrong one, and
#   never fails the emission.
#
#   A PR matched by several searches appears ONCE, with its signals merged
#   (unioned) — the dedup groups by URL and unions the signal arrays.
#
#   Bot-authored PRs are INCLUDED (Renovate/Dependabot are no longer excluded);
#   they get tag "dependabot". Human-review PRs get tag "pr-review". Draft
#   PRs are INCLUDED, with a "draft" label appended so the TUI card shows it.
#
# Output format (FeedItem):
#   [{"external_id":"review:org/repo#42","title":"#42 PR title","description":"...","url":"...","status":"backlog","tag":"pr-review","labels":["@author","repo","ci:pass"],"signals":["team-request","reviewed"]}]
#
# Routing is handled by dispatch, not here. The signal vocabulary is the wire
# contract with the role router (see docs/specs/feeds.allium, enum Signal).

set -euo pipefail

# ---------------------------------------------------------------------------
# Repositories to search for the review-related queries: edit repos.conf in
# the same directory (the REPOS array — SSOT shared with fetch-cve.sh). Falls
# back to skipping those queries when repos.conf is absent or lists no repos.
REPOS=()

# Organisations to search for the org-scoped review queries ONLY: edit
# org.conf in the same directory (the ORGS array). Falls back to skipping
# those queries when org.conf is absent or lists no orgs.
ORGS=()

# Bot author logins for the bot-author pass ONLY: edit bots.conf in the same
# directory (the BOT_AUTHORS array), using gh search's app form, e.g.
# "app/kognic-renovate". Falls back to skipping that pass when bots.conf is
# absent or lists no authors.
BOT_AUTHORS=()

# Node ids per batched CI-status query. GraphQL `nodes(ids: [...])` caps at 100;
# 50 leaves headroom and keeps the argument list well inside any command-line
# limit. Raising it reduces calls per poll, never correctness.
CI_BATCH=50

# Per-repo cap on the bot-author pass. Applied to each (repo x author) query
# individually — see search_bot_prs for why it is not one capped multi-repo
# query.
BOT_LIMIT=20

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [[ -f "$SCRIPT_DIR/repos.conf" ]]; then
  # shellcheck source=repos.conf
  source "$SCRIPT_DIR/repos.conf"
fi
if [[ -f "$SCRIPT_DIR/org.conf" ]]; then
  # shellcheck source=org.conf
  source "$SCRIPT_DIR/org.conf"
fi
if [[ -f "$SCRIPT_DIR/bots.conf" ]]; then
  # shellcheck source=bots.conf
  source "$SCRIPT_DIR/bots.conf"
fi
# ---------------------------------------------------------------------------

repo_flags=()
for repo in "${REPOS[@]}"; do
  repo_flags+=(--repo "$repo")
done

owner_flags=()
for org in "${ORGS[@]}"; do
  owner_flags+=(--owner "$org")
done

# The gh user's login, for the author-me signal. Soft-fails to empty so a
# transient `gh api` error degrades author-me detection rather than the feed.
ME="$(gh api user -q .login 2>/dev/null || true)"

# Run one `gh search prs` query for the given qualifier, scoped by the given
# scope flags (repo_flags or owner_flags), and print a FeedItem JSON array on
# stdout, tagging every PR with the supplied signal plus any per-PR author
# signals. Usage: search_prs <qualifier> <signal> <scope_flags_name>
search_prs() {
  local qualifier="$1"
  local signal="$2"
  local -n scope_flags="$3"
  local raw

  if [[ ${#scope_flags[@]} -eq 0 ]]; then
    echo "[]"
    return 0
  fi

  # `$qualifier` is one or more bare GitHub search terms (e.g.
  # "review-requested:@me" or "commenter:@me -author:@me"). They go AFTER `--`
  # so a leading-dash term like `-author:@me` isn't parsed as a gh flag, and
  # are deliberately left unquoted so a multi-term qualifier word-splits into
  # separate search terms instead of one mangled `commenter:"@me -author:@me"`.
  # Capture stdout only; let gh's stderr flow to the feed log so a warning
  # on a successful exit can't corrupt the JSON we hand to jq.
  # shellcheck disable=SC2086  # intentional word-splitting of $qualifier
  if ! raw=$(gh search prs \
    --state=open \
    "${scope_flags[@]}" \
    --json id,number,title,body,url,repository,isDraft,author \
    --limit 100 \
    -- $qualifier); then
    echo "fetch-reviews: gh search prs ($qualifier) failed" >&2
    echo "[]"
    return 0
  fi

  printf '%s' "$raw" | to_feed_items "$signal"
}

# Map a `gh search prs --json …` array on stdin to a FeedItem array on stdout,
# tagging every PR with $1 plus any per-PR author signals. Shared by the
# review passes and the bot-author pass so both emit an identical item shape.
to_feed_items() {
  local signal="$1"

  jq --arg signal "$signal" --arg me "$ME" '[
    .[] |
    (.author.login // "") as $login |
    ($login | test("\\[bot\\]$")) as $is_bot |
    {
      # Transient: the GraphQL node id of this PR, used to key the batched
      # CI-status lookup and STRIPPED before the array is emitted.
      # Underscore-prefixed so it cannot be mistaken for a FeedItem field.
      # NOTE: no apostrophes in this jq program -- it sits inside a
      # single-quoted bash string, and one would close it.
      _pr_id: .id,
      external_id: ("review:" + .repository.nameWithOwner + "#" + (.number | tostring)),
      title: ("#" + (.number | tostring) + " " + .title),
      description: ((.body // "") | .[0:500]),
      url: .url,
      status: "backlog",
      tag: (if $is_bot then "dependabot" else "pr-review" end),
      labels: (
        (if $login != "" then ["@\($login)"] else [] end)
        + [.repository.name]
        + (if .isDraft then ["draft"] else [] end)
      ),
      signals: (
        [$signal]
        + (if $is_bot then ["author-bot"] else [] end)
        + (if ($me != "" and $login == $me) then ["author-me"] else [] end)
      )
    }
  ]'
}

# Bot-author pass: emit every open PR authored by one of BOT_AUTHORS in one of
# REPOS, whether or not you are involved in reviewing it. Signal is
# "author-bot", which dispatch's router maps to the Bots sub-epic.
#
# ONE query per (repo x author), deliberately: --limit caps a single gh search
# call, so one multi-repo call would cap the pass as a WHOLE and let a repo
# with a large bot backlog starve every other repo out of the emission. The
# cost is len(REPOS) * len(BOT_AUTHORS) calls per poll. Newest-first so the cap
# keeps the freshest bumps.
search_bot_prs() {
  local repo author raw

  if [[ ${#BOT_AUTHORS[@]} -eq 0 || ${#REPOS[@]} -eq 0 ]]; then
    echo "[]"
    return 0
  fi

  for repo in "${REPOS[@]}"; do
    for author in "${BOT_AUTHORS[@]}"; do
      # `author:<login>` goes after `--` for the same reason the review
      # qualifiers do. Quoted here: it is always a single term.
      if ! raw=$(gh search prs \
        --state=open \
        --repo "$repo" \
        --json id,number,title,body,url,repository,isDraft,author \
        --limit "$BOT_LIMIT" \
        --sort created \
        --order desc \
        -- "author:$author"); then
        echo "fetch-reviews: gh search prs (author:$author in $repo) failed" >&2
        continue
      fi

      printf '%s' "$raw" | to_feed_items "author-bot"
    done
  done
}

# GraphQL for the batched CI-status lookup. One request covers up to CI_BATCH
# PRs. `commits(last:1)` is the PR's head commit; its statusCheckRollup is null
# when nothing ran, which is a distinct outcome from any state and must stay
# distinct (no label, not "pass").
CI_QUERY='query($ids:[ID!]!){nodes(ids:$ids){... on PullRequest{id commits(last:1){nodes{commit{statusCheckRollup{state}}}}}}}'

# Map the deduped FeedItem array on $1 to a JSON object {node_id: ci_label} on
# stdout, batching CI_BATCH ids per `gh api graphql` call.
#
# Every failure mode degrades to a MISSING entry, never a wrong one: a failed
# request, an unresolvable id, a null rollup and an upstream state this script
# does not recognise all leave the PR with no ci label. That is the honest
# reading — "we do not know" looks like "nothing ran", and both are better than
# a green badge on a red PR.
fetch_ci_labels() {
  local items="$1" acc='{}' raw i j
  local -a ids=() args=()

  # `|| true`: an items array with no _pr_id (nothing emitted) is not an error.
  mapfile -t ids < <(printf '%s' "$items" | jq -r '.[]._pr_id // empty' || true)
  if [[ ${#ids[@]} -eq 0 ]]; then
    printf '%s' "$acc"
    return 0
  fi

  for ((i = 0; i < ${#ids[@]}; i += CI_BATCH)); do
    args=()
    for ((j = i; j < i + CI_BATCH && j < ${#ids[@]}; j++)); do
      args+=(-F "ids[]=${ids[j]}")
    done

    if ! raw=$(gh api graphql -f query="$CI_QUERY" "${args[@]}"); then
      echo "fetch-reviews: gh api graphql (ci status) failed" >&2
      continue
    fi

    acc=$(jq -n --argjson acc "$acc" --argjson raw "$raw" '
      $acc + ([
        $raw.data.nodes[]?
        | select(. != null and .id != null)
        | {
            key: .id,
            value: (
              .commits.nodes[0].commit.statusCheckRollup.state
              | if   . == "SUCCESS"                    then "ci:pass"
                elif . == "FAILURE" or . == "ERROR"    then "ci:fail"
                elif . == "PENDING" or . == "EXPECTED" then "ci:pending"
                else null end
            )
          }
        | select(.value != null)
      ] | from_entries)
    ')
  done

  printf '%s' "$acc"
}

# Run every search, then dedup by URL MERGING the signal arrays (a PR matched
# by several queries keeps all its signals). NOT unique_by, which would drop
# all but one object and lose the other queries' signals.
deduped=$({
  search_prs "review-requested:@me" "team-request" repo_flags
  search_prs "user-review-requested:@me" "direct-request" repo_flags
  search_prs "reviewed-by:@me" "reviewed" repo_flags
  search_prs "commenter:@me -author:@me" "commented" repo_flags
  # Three of the four qualifiers again, org-scoped — every match here is
  # tagged with one shared signal so it always lands in My Reviews (never
  # Team/Bots), regardless of which qualifier matched. review-requested:@me
  # is deliberately EXCLUDED from this org-scoped pass: unlike the other
  # three (which are always about ME personally), it also matches PRs
  # requested from a TEAM I belong to, and org-wide that would sweep in
  # far more team-request noise than repo-scoped ever did.
  search_prs "user-review-requested:@me" "org-review" owner_flags
  search_prs "reviewed-by:@me" "org-review" owner_flags
  search_prs "commenter:@me -author:@me" "org-review" owner_flags
  # Bot-authored PRs regardless of review involvement, repo-scoped only.
  search_bot_prs
} | jq -s 'add
  | group_by(.url)
  | map(.[0] + {signals: (map(.signals[]) | unique)})')

# Attach the CI label and strip the transient node id. `del` runs on every item
# whether or not it got a label, so _pr_id never reaches the wire format.
printf '%s' "$deduped" | jq --argjson ci "$(fetch_ci_labels "$deduped")" '
  map(
    (._pr_id // "") as $id
    | del(._pr_id)
    | if $ci[$id] then .labels += [$ci[$id]] else . end
  )'
