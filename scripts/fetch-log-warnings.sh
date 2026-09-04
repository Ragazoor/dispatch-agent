#!/usr/bin/env bash
# Scan a tracing-formatted log file and emit one FeedItem per DISTINCT
# WARN/ERROR record, for use as a dispatch feed epic command.
#
# Requirements: jq, awk
#
# Usage:
#   dispatch verify-feed scripts/fetch-log-warnings.sh   # validate output
#
# THIS FEED'S EPIC MUST BE APPEND-ONLY. A log record is an EVENT: it happens
# once, and nothing upstream ever retracts it, so its absence from a later
# emission means nothing at all. An ordinary feed epic reads absence as
# "closed" and would delete the task — and tear down its worktree — on the very
# next poll. Set the flag once, when you wire the epic up:
#
#   update_epic(epic_id: <id>, feed_append_only: true)
#
# See docs/specs/feeds.allium: AppendOnlyFeed.
#
# HOW A CARD IS CLOSED. Three outcomes, all three permanent:
#
#   1. A real bug         -> fix it.
#   2. A false positive   -> the code is right and the LOG LINE is wrong.
#                            DEMOTE it to INFO/DEBUG: the record stops being
#                            emitted and the log's WARN level gets more honest.
#                            Rewording instead is NOT terminal — the message
#                            head is the card's identity, so a reworded line
#                            arrives as a new card to confirm and archive once.
#   3. Real but transient -> upstream flakiness you still want logged.
#                            ARCHIVE the card.
#
# Archive, never delete. An archived task keeps its external id, so the feed
# matches it on every later poll and creates nothing. A DELETED task takes that
# id with it, and the next poll inserts the card all over again.
#
# Note: when used as a dispatch feed_command, use the absolute path to this
# script. Relative paths only work if dispatch is launched from the project root.
#
# Note: the feed owns `description` and rewrites it on every poll — that is how
# the occurrence count stays current while you work. Do not keep triage notes
# there; they will be overwritten. Use the task's plan.

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration: edit log-warnings.conf in the same directory (SSOT), or set
# these directly below as a fallback when that file is not present.
#
# CONFIGURE A COPY, NOT THE TRACKED FILE. Copy this script and its conf next to
# your other live feed scripts (<data_dir>/scripts/, alongside repos.conf) and
# edit the copy — editing the tracked pair in the repo leaves a permanent local
# diff. The conf is sourced from whatever directory the script itself is in, so
# the two travel together.
#
#   LOG_FILE  absolute path to the log to scan. Empty = this script is inert.
#   REPO_URL  GitHub root URL of the repo the log belongs to. Dispatch resolves
#             it to a local clone so the created task can be dispatched into a
#             worktree. A log record carries no URL of its own; resolving a
#             repo is the only reason this field is set.
#   LEVELS    which severities become cards, as an ERE alternation.
#   SAMPLES   how many raw lines to quote in the description.
LOG_FILE=""
REPO_URL=""
LEVELS="WARN|ERROR"
SAMPLES=3

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [[ -f "$SCRIPT_DIR/log-warnings.conf" ]]; then
  # shellcheck source=/dev/null
  source "$SCRIPT_DIR/log-warnings.conf"
fi
# ---------------------------------------------------------------------------

if [[ -z "$LOG_FILE" ]]; then
  echo '[]'
  exit 0
fi

if [[ ! -r "$LOG_FILE" ]]; then
  echo "error: log file not readable: $LOG_FILE" >&2
  echo '[]'
  exit 1
fi

# WHAT MAKES TWO LINES THE SAME PROBLEM.
#
# Not the raw line: every line differs by timestamp, and a real log holds
# six-figure counts of a handful of distinct problems. Each record is
# fingerprinted as its emitting MODULE TARGET plus the STATIC HEAD of its
# message — everything up to the first ":" or the first " key=", which is where
# tracing's literal format string stops and the interpolated detail begins.
#
# That is a proxy for the CALL SITE. A file:line would be exact, but it moves
# on every refactor, and a moved fingerprint means a card you already triaged
# and archived comes back as new. The static head survives refactoring.
#
# Two normalisations inside the head, because a message can interpolate before
# it reaches a colon: digit runs collapse to N, and org/repo pairs to R.
#
# Pointing this at a different repo's logs means editing this awk block. The
# format it parses is tracing's; the fingerprint recipe is policy, and policy
# belongs in the script rather than in the dispatch runtime.
# AGGREGATE IN AWK, NOT IN JQ. awk emits one row PER GROUP, not per line, so
# everything downstream is proportional to the number of distinct records
# rather than to the size of the log. That matters: handing jq one object per
# matched line peaked at 1.6 GB of resident memory on a 335 MB log, because
# 72% of the bytes were sample text for lines that were never quoted. This
# shape holds flat at a few MB on the same input, and the full re-scan — which
# is what makes archive-as-suppression work — is kept.
grep -aE " ($LEVELS) " -- "$LOG_FILE" \
| awk -v samples="$SAMPLES" '
{
    gsub(/\t/, " ")
    level = $2
    target = $3
    sub(/:$/, "", target)

    # The message is whatever follows "<target>: ".
    at = index($0, $3)
    head = substr($0, at + length($3) + 1)

    sub(/:.*/, "", head)              # cut at the first colon
    sub(/ [a-z_]+=.*/, "", head)      # cut at the first structured field
    gsub(/[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+/, "R", head)
    gsub(/[0-9]+/, "N", head)
    gsub(/^ +| +$/, "", head)
    if (head == "") head = "(no message)"

    key = level "\t" target "\t" head
    if (!(key in count)) {
        count[key] = 0
        first[key] = $1
        last[key] = $1
    }
    count[key]++

    # ISO-8601 timestamps sort lexically, so a running min/max costs O(1) per
    # line and assumes nothing about the order lines appear in the file.
    if ($1 < first[key]) first[key] = $1
    if ($1 > last[key])  last[key] = $1

    # Truncation is paid only for the lines actually quoted, not for all of them.
    # Samples are joined with US (0x1f), never a newline: the stream below is
    # one line per group, so a newline here would split a group across rows.
    if (count[key] <= samples) {
        sample[key] = (count[key] == 1 ? "" : sample[key] "\037") "    " substr($0, 1, 300)
    }
}
END {
    for (key in count) {
        print key "\t" count[key] "\t" first[key] "\t" last[key] "\t" sample[key]
    }
}' \
| jq -Rs --arg repo_url "$REPO_URL" '
    [ split("\n")[] | select(length > 0) | split("\t")
      | {level: .[0], target: .[1], head: .[2],
         count: (.[3] | tonumber), first: .[4], last: .[5], samples: .[6]} ]
    # Rank by how often the record fired: sort_order is ascending, so the
    # noisiest record gets rank 1 and lands at the top of the column.
    | sort_by(-.count)
    | to_entries
    | map(
        .value as $f
        | ($f.target | split("::") | .[-2:] | join("::")) as $short
        | {
            external_id: "log:\($f.level):\($f.target):\($f.head)",
            title: "[\($f.level)] \($short): \($f.head)",
            description: (
              "\($f.count) occurrence(s), first \($f.first), last \($f.last).\n"
              + "\nModule: \($f.target)\nLevel:  \($f.level)\n"
              + "\nTriage this record. It is exactly one of three things.\n"
              + "\n1. A REAL BUG. Fix it.\n"
              + "\n2. A FALSE POSITIVE: the code is behaving correctly and the log\n"
              + "   line is what is wrong — it warns about something expected, or\n"
              + "   its wording is ambiguous about which. Run /weed first: if the\n"
              + "   spec and the code disagree about this path, that disagreement\n"
              + "   is the actual finding and the log line is only its symptom.\n"
              + "   DEMOTING the line to INFO/DEBUG retires this card outright.\n"
              + "   REWORDING it does not: the message head is this card'"'"'s\n"
              + "   identity, so a reworded line arrives as a NEW card. That is\n"
              + "   working as intended — archive this one, then confirm and\n"
              + "   archive its replacement once.\n"
              + "\n3. REAL BUT TRANSIENT — upstream flakiness that is worth keeping\n"
              + "   at this level. ARCHIVE this card. Archiving is permanent: the\n"
              + "   feed will never recreate it. Do NOT delete it; a deleted card\n"
              + "   comes back on the next poll.\n"
              + "\nSample lines:\n"
              + ($f.samples | split("\u001f") | join("\n"))
            ),
            url: $repo_url,
            status: "backlog",
            tag: "bug",
            labels: [($f.level | ascii_downcase)],
            sort_order: (.key + 1)
          })
'
