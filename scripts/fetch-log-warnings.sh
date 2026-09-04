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
#                            Demote it to INFO/DEBUG, or reword it. The record
#                            stops being emitted, and the log's WARN level gets
#                            more honest.
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
grep -aE " ($LEVELS) " -- "$LOG_FILE" \
| awk '
{
    gsub(/\t/, " ")
    level = $2
    target = $3
    sub(/:$/, "", target)

    # The message is whatever follows "<target>: ".
    at = index($0, $3)
    msg = substr($0, at + length($3) + 1)

    head = msg
    sub(/:.*/, "", head)              # cut at the first colon
    sub(/ [a-z_]+=.*/, "", head)      # cut at the first structured field
    gsub(/[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+/, "R", head)
    gsub(/[0-9]+/, "N", head)
    gsub(/^ +| +$/, "", head)
    if (head == "") head = "(no message)"

    print level "\t" target "\t" head "\t" $1 "\t" substr($0, 1, 300)
}' \
| jq -Rs --arg repo_url "$REPO_URL" --argjson samples "$SAMPLES" '
    [ split("\n")[] | select(length > 0) | split("\t")
      | {level: .[0], target: .[1], head: .[2], ts: .[3], line: .[4]} ]
    | group_by(.level + " " + .target + " " + .head)
    | map(
        (.[0]) as $f
        | length as $count
        | (map(.ts) | sort) as $times
        | ($f.target | split("::") | .[-2:] | join("::")) as $short
        | {
            external_id: "log:\($f.level):\($f.target):\($f.head)",
            title: "[\($f.level)] \($short): \($f.head)",
            description: (
              "\($count) occurrence(s), first \($times[0]), last \($times[-1]).\n"
              + "\nModule: \($f.target)\nLevel:  \($f.level)\n"
              + "\nTriage this record. It is exactly one of three things.\n"
              + "\n1. A REAL BUG. Fix it.\n"
              + "\n2. A FALSE POSITIVE: the code is behaving correctly and the log\n"
              + "   line is what is wrong — it warns about something expected, or\n"
              + "   its wording is ambiguous about which. Demote it to INFO/DEBUG,\n"
              + "   or reword it so it is unambiguous. Run /weed first: if the spec\n"
              + "   and the code disagree about this path, that disagreement is the\n"
              + "   actual finding and the log line is only its symptom.\n"
              + "\n3. REAL BUT TRANSIENT — upstream flakiness that is worth keeping\n"
              + "   at this level. ARCHIVE this card. Archiving is permanent: the\n"
              + "   feed will never recreate it. Do NOT delete it; a deleted card\n"
              + "   comes back on the next poll.\n"
              + "\nSample lines:\n"
              + (map("    " + .line) | .[0:$samples] | join("\n"))
            ),
            url: $repo_url,
            status: "backlog",
            tag: "bug",
            labels: [($f.level | ascii_downcase)],
            _count: $count
          })
    # Rank by how often the record fired, noisiest first: sort_order is
    # ascending, so the rank is 1-based rather than a negative count.
    | sort_by(-._count)
    | to_entries
    | map(.value + {sort_order: (.key + 1)} | del(._count))
'
