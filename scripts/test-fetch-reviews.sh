#!/usr/bin/env bash
# test-fetch-reviews.sh — stub-gh shell test for scripts/fetch-reviews.sh.
#
# Puts a fake `gh` first on PATH that returns canned JSON per search qualifier,
# runs fetch-reviews.sh against it, and asserts the single-emission +
# signal-merging contract:
#   - a PR matched by two queries collapses to ONE item carrying BOTH signals
#   - bot-authored PRs (renovate/dependabot) are included with author-bot +
#     tag "dependabot" (no longer excluded)
#   - a PR authored by the gh user carries the author-me signal
#   - draft PRs are included, with a "draft" label; non-draft PRs get no such
#     label
#   - the output parses as a JSON array
#   - a PR matched ONLY by an org-scoped review query (via org.conf) carries
#     the org-review signal
#   - a bot PR matched ONLY by the bot-author pass (via bots.conf) carries
#     author-bot and nothing else, so route() sends it to Bots
#   - a bot PR matched by BOTH the bot-author pass and a review query merges
#     into one item carrying both signals (engagement precedence preserved)
#   - the bot-author pass is capped PER REPO: one query per (repo x author),
#     with --limit 20 --sort created --order desc
#   - the bot-author pass is skipped entirely when bots.conf is absent
#
# Run from the repo root:  bash scripts/test-fetch-reviews.sh
# Exits 0 on success, non-zero with a diagnostic on the first failed assertion.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REVIEWS_SCRIPT="$SCRIPT_DIR/fetch-reviews.sh"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

# --- Fake gh: dispatch on the search qualifier in its arguments. -----------
# NOTE: "user-review-requested:@me" contains "review-requested:@me" as a
# substring, so it MUST be matched first.
cat >"$WORKDIR/gh" <<'STUB'
#!/usr/bin/env bash
args="$*"

if [[ "$args" == *"api user"* ]]; then
  printf '%s\n' "ragge"
  exit 0
fi

# Record every search invocation so the test can assert the per-repo cap and
# sort flags the bot-author pass must use.
printf '%s\n' "$args" >>"$(dirname "$0")/gh-args.log"

# Bot-author pass. Checked before the review qualifiers: its queries also
# carry --repo flags, so it would otherwise fall through to them. The
# `author:app/` prefix cannot collide with `commenter:@me -author:@me`.
if [[ "$args" == *"author:app/"* ]]; then
  if [[ "$args" == *"author:app/testbot"* ]]; then
    # Matched by NO review query, so the test can assert it lands with ONLY
    # the author-bot signal.
    cat <<'JSON'
[
  {"number":9,"title":"Bump unrelated dep","body":"","url":"https://github.com/testorg/repo/pull/9","repository":{"name":"repo","nameWithOwner":"testorg/repo"},"isDraft":false,"author":{"login":"testbot[bot]"}}
]
JSON
  elif [[ "$args" == *"author:app/dependabot"* ]]; then
    # PR3 is ALSO returned by reviewed-by:@me below, so the test can assert
    # the two passes merge into one item carrying both signals.
    cat <<'JSON'
[
  {"number":3,"title":"Bump lib","body":"","url":"https://github.com/testorg/repo/pull/3","repository":{"name":"repo","nameWithOwner":"testorg/repo"},"isDraft":false,"author":{"login":"dependabot[bot]"}}
]
JSON
  else
    printf '%s\n' '[]'
  fi
  exit 0
fi

if [[ "$args" == *"--owner"* ]]; then
  # Org-scoped pass. "user-review-requested:@me" must be checked before the
  # bare "review-requested:@me" (substring of the former).
  if [[ "$args" == *"user-review-requested:@me"* ]]; then
    printf '%s\n' '[]'
  elif [[ "$args" == *"reviewed-by:@me"* ]]; then
    # Exclusive to this scope, so the test can assert it lands with ONLY
    # the org-review signal (no team-request/direct-request/reviewed/
    # commented).
    cat <<'JSON'
[
  {"number":7,"title":"Org-scoped review","body":"","url":"https://github.com/otherorg/repo/pull/7","repository":{"name":"repo","nameWithOwner":"otherorg/repo"},"isDraft":false,"author":{"login":"dave"}}
]
JSON
  elif [[ "$args" == *"review-requested:@me"* ]]; then
    # This qualifier is team-inclusive and must NEVER be run org-scoped —
    # if fetch-reviews.sh regresses and calls it anyway, this PR would leak
    # into the output and the "PR8 never appears" assertion below catches it.
    cat <<'JSON'
[
  {"number":8,"title":"Team-only org PR","body":"","url":"https://github.com/otherorg/repo/pull/8","repository":{"name":"repo","nameWithOwner":"otherorg/repo"},"isDraft":false,"author":{"login":"eve"}}
]
JSON
  else
    printf '%s\n' '[]'
  fi
  exit 0
fi

if [[ "$args" == *"user-review-requested:@me"* ]]; then
  printf '%s\n' '[]'
elif [[ "$args" == *"review-requested:@me"* ]]; then
  cat <<'JSON'
[
  {"number":1,"title":"Add feature","body":"d","url":"https://github.com/testorg/repo/pull/1","repository":{"name":"repo","nameWithOwner":"testorg/repo"},"isDraft":false,"author":{"login":"alice"}},
  {"number":2,"title":"Bump dep","body":"","url":"https://github.com/testorg/repo/pull/2","repository":{"name":"repo","nameWithOwner":"testorg/repo"},"isDraft":false,"author":{"login":"kognic-renovate[bot]"}},
  {"number":5,"title":"Draft PR","body":"","url":"https://github.com/testorg/repo/pull/5","repository":{"name":"repo","nameWithOwner":"testorg/repo"},"isDraft":true,"author":{"login":"bob"}}
]
JSON
elif [[ "$args" == *"reviewed-by:@me"* ]]; then
  cat <<'JSON'
[
  {"number":1,"title":"Add feature","body":"d","url":"https://github.com/testorg/repo/pull/1","repository":{"name":"repo","nameWithOwner":"testorg/repo"},"isDraft":false,"author":{"login":"alice"}},
  {"number":3,"title":"Bump lib","body":"","url":"https://github.com/testorg/repo/pull/3","repository":{"name":"repo","nameWithOwner":"testorg/repo"},"isDraft":false,"author":{"login":"dependabot[bot]"}}
]
JSON
elif [[ "$args" == *"commenter:@me"* ]]; then
  cat <<'JSON'
[
  {"number":4,"title":"My own PR","body":"","url":"https://github.com/testorg/repo/pull/4","repository":{"name":"repo","nameWithOwner":"testorg/repo"},"isDraft":false,"author":{"login":"ragge"}}
]
JSON
else
  printf '%s\n' '[]'
fi
STUB
chmod +x "$WORKDIR/gh"

# --- Script copy + sibling repos.conf/org.conf/bots.conf so all scopes query.
cp "$REVIEWS_SCRIPT" "$WORKDIR/fetch-reviews.sh"
chmod +x "$WORKDIR/fetch-reviews.sh"
echo 'REPOS=("testorg/repo")' >"$WORKDIR/repos.conf"
echo 'ORGS=("testorg")' >"$WORKDIR/org.conf"
echo 'BOT_AUTHORS=("app/testbot" "app/dependabot")' >"$WORKDIR/bots.conf"

output="$(PATH="$WORKDIR:$PATH" bash "$WORKDIR/fetch-reviews.sh")"

fail() {
  echo "test-fetch-reviews: FAIL — $1" >&2
  echo "---- output ----" >&2
  printf '%s\n' "$output" >&2
  exit 1
}

assert() {
  local desc="$1" filter="$2"
  printf '%s' "$output" | jq -e "$filter" >/dev/null 2>&1 || fail "$desc"
}

# Output is a JSON array.
assert "output is a JSON array" 'type == "array"'

# Exactly seven PRs survive (PR1 deduped across two queries; draft PR5 now
# included; PR7 added by the org-scoped reviewed-by:@me query; PR9 added by
# the bot-author pass; PR3 returned by both reviewed-by and the bot-author
# pass collapses to one).
assert "exactly 7 items after dedup" 'length == 7'

# PR1 matched by review-requested AND reviewed-by -> one item, both signals.
assert "PR1 carries team-request" \
  'map(select(.url | endswith("/pull/1"))) | .[0].signals | index("team-request")'
assert "PR1 carries reviewed" \
  'map(select(.url | endswith("/pull/1"))) | .[0].signals | index("reviewed")'
assert "PR1 appears exactly once" \
  '[.[] | select(.url | endswith("/pull/1"))] | length == 1'
assert "PR1 keeps tag pr-review" \
  'map(select(.url | endswith("/pull/1"))) | .[0].tag == "pr-review"'

# Renovate bot PR included, author-bot + tag dependabot.
assert "renovate PR2 tagged dependabot" \
  'map(select(.url | endswith("/pull/2"))) | .[0].tag == "dependabot"'
assert "renovate PR2 carries author-bot" \
  'map(select(.url | endswith("/pull/2"))) | .[0].signals | index("author-bot")'

# Dependabot bot PR included, author-bot + tag dependabot.
assert "dependabot PR3 tagged dependabot" \
  'map(select(.url | endswith("/pull/3"))) | .[0].tag == "dependabot"'
assert "dependabot PR3 carries author-bot" \
  'map(select(.url | endswith("/pull/3"))) | .[0].signals | index("author-bot")'

# Self-authored PR carries author-me (so route() keeps it out of My Reviews).
assert "self-authored PR4 carries author-me" \
  'map(select(.url | endswith("/pull/4"))) | .[0].signals | index("author-me")'
assert "self-authored PR4 carries commented" \
  'map(select(.url | endswith("/pull/4"))) | .[0].signals | index("commented")'

# Draft PR5 is included, and carries a "draft" label.
assert "draft PR5 included" \
  '[.[] | select(.url | endswith("/pull/5"))] | length == 1'
assert "draft PR5 carries draft label" \
  'map(select(.url | endswith("/pull/5"))) | .[0].labels | index("draft")'

# Non-draft PR1 does NOT carry a draft label.
assert "non-draft PR1 has no draft label" \
  'map(select(.url | endswith("/pull/1"))) | (.[0].labels | index("draft")) == null'

# PR7 matched only by the org-scoped reviewed-by:@me query carries
# org-review and ONLY org-review (no repo-scoped signal leaked in).
assert "org-scoped-only PR7 carries org-review" \
  'map(select(.url | endswith("/pull/7"))) | .[0].signals | index("org-review")'
assert "org-scoped-only PR7 carries no other signal" \
  'map(select(.url | endswith("/pull/7"))) | .[0].signals == ["org-review"]'
assert "org-scoped-only PR7 keeps tag pr-review" \
  'map(select(.url | endswith("/pull/7"))) | .[0].tag == "pr-review"'

# PR8 is only returned by an org-scoped review-requested:@me call — a
# qualifier fetch-reviews.sh must NEVER run org-scoped (it also matches
# team-based requests). If it ever regresses and calls it, PR8 leaks in.
assert "team-inclusive org-scoped query never runs (PR8 absent)" \
  '[.[] | select(.url | endswith("/pull/8"))] | length == 0'

# --- Bot-author pass (bots.conf) -------------------------------------------

# PR9 is matched by NO review query — only by author:app/testbot. It must
# carry author-bot and ONLY author-bot, which is what makes route() send it to
# Bots (RouteSignals rule 2) rather than the my_reviews fallback.
assert "bot-only PR9 included" \
  '[.[] | select(.url | endswith("/pull/9"))] | length == 1'
assert "bot-only PR9 carries author-bot" \
  'map(select(.url | endswith("/pull/9"))) | .[0].signals | index("author-bot")'
assert "bot-only PR9 carries no other signal" \
  'map(select(.url | endswith("/pull/9"))) | .[0].signals == ["author-bot"]'
assert "bot-only PR9 tagged dependabot" \
  'map(select(.url | endswith("/pull/9"))) | .[0].tag == "dependabot"'

# PR3 is returned by BOTH reviewed-by:@me and author:app/dependabot. It must
# collapse to one item carrying both signals — engagement then wins over the
# bot rule in route(), keeping a bot PR the user reviewed in My Reviews.
assert "PR3 appears exactly once across both passes" \
  '[.[] | select(.url | endswith("/pull/3"))] | length == 1'
assert "PR3 carries reviewed" \
  'map(select(.url | endswith("/pull/3"))) | .[0].signals | index("reviewed")'
assert "PR3 carries author-bot" \
  'map(select(.url | endswith("/pull/3"))) | .[0].signals | index("author-bot")'

# The cap is PER REPO, not per query: every bot-author call carries exactly
# one --repo, --limit 20, and newest-first sorting. A single multi-repo call
# with one --limit would let a busy repo starve the others.
bot_calls="$(grep -c 'author:app/' "$WORKDIR/gh-args.log" || true)"
[[ "$bot_calls" == "2" ]] ||
  fail "expected 2 bot-author calls (1 repo x 2 authors), got $bot_calls"
while read -r line; do
  [[ "$line" == *"--limit 20"* ]] ||
    fail "bot-author call missing --limit 20: $line"
  [[ "$line" == *"--sort created"* && "$line" == *"--order desc"* ]] ||
    fail "bot-author call missing newest-first sort: $line"
  repo_count="$(grep -o -- '--repo' <<<"$line" | wc -l)"
  [[ "$repo_count" == "1" ]] ||
    fail "bot-author call must scope ONE repo, got $repo_count: $line"
done < <(grep 'author:app/' "$WORKDIR/gh-args.log")

# --- Bot-author pass is inert without bots.conf ----------------------------

rm "$WORKDIR/bots.conf"
: >"$WORKDIR/gh-args.log"
output="$(PATH="$WORKDIR:$PATH" bash "$WORKDIR/fetch-reviews.sh")"

assert "without bots.conf, back to 6 items" 'length == 6'
assert "without bots.conf, PR9 absent" \
  '[.[] | select(.url | endswith("/pull/9"))] | length == 0'
# PR3 still carries author-bot — that signal comes from the author login, not
# from the bot-author pass, and is what kept bot PRs routable before this pass
# existed. (jq's `unique` sorts, hence the ordering here.)
assert "without bots.conf, PR3 keeps its login-derived author-bot" \
  'map(select(.url | endswith("/pull/3"))) | .[0].signals == ["author-bot","reviewed"]'

if grep -q 'author:app/' "$WORKDIR/gh-args.log"; then
  fail "bot-author pass ran without bots.conf"
fi

echo "test-fetch-reviews: all assertions passed"
