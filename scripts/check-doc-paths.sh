#!/usr/bin/env bash
# Verify every `src/…` and `docs/…` path mentioned in our agent-facing docs
# actually exists, and that every `file:NN` line citation is in range. By
# default scans README.md and CLAUDE.md plus every docs/*.md and
# docs/specs/*.allium. The globs are deliberately non-recursive, which
# excludes docs/plans/,
# docs/superpowers/, and docs/research/ — those are dated artifacts, so a
# reference that has since gone stale is expected, not a defect. Pass an
# explicit path to scan a single file instead.
#
# What is validated, per reference:
#   - plain paths (`src/db/mod.rs`, `docs/conventions.md`, `docs/specs/x.allium`)
#   - directory references written with a trailing slash (`src/tui/ui/kanban/`)
#   - brace lists (`src/db/queries/{tasks,epics}.rs`) — every member is checked
#   - line citations (`src/db/mod.rs:30`, `src/db/mod.rs:30-42`) — the file must
#     have at least that many lines
#
# Behaviour is pinned by scripts/test-check-doc-paths.sh.
# Run from the repo root. Exits non-zero if anything is stale.
set -euo pipefail

if [[ $# -gt 0 ]]; then
    DOCS=("$@")
else
    # Globbed, not hand-listed: a doc added under docs/ is covered the moment it
    # lands, with nothing to remember. nullglob so an empty match expands to
    # nothing rather than to a literal `docs/*.md` that then "does not exist".
    # README.md is the repo's front page and the one doc written for people who
    # have not cloned yet, so a dead link there costs the most. It sat outside
    # this list until #4501, which is how three stale docs/images/ references
    # survived a rename.
    DOCS=(README.md CLAUDE.md)
    shopt -s nullglob
    DOCS+=(docs/*.md docs/specs/*.allium)
    shopt -u nullglob
fi

# A path-ish token under src/ or docs/, ending in a known extension or a
# trailing slash, optionally followed by a `:NN` / `:NN-MM` line citation.
PATH_RE='(src|docs)/[A-Za-z0-9_{},./-]*(\.rs|\.md|\.allium|\.sh|/)(:[0-9]+(-[0-9]+)?)?'

# Expand a single `{a,b,c}` brace group into one line per member. Tokens
# without a brace group pass through unchanged. Deliberately hand-rolled
# rather than eval'd — doc text is not trusted shell input.
expand_braces() {
    local token="$1"
    if [[ "$token" != *'{'*'}'* ]]; then
        printf '%s\n' "$token"
        return
    fi

    local prefix="${token%%\{*}"
    local rest="${token#*\{}"
    local list="${rest%%\}*}"
    local suffix="${rest#*\}}"

    local -a items
    IFS=',' read -ra items <<<"$list"
    local item
    for item in "${items[@]}"; do
        [[ -n "$item" ]] && printf '%s%s%s\n' "$prefix" "$item" "$suffix"
    done
}

problems=0
for DOC in "${DOCS[@]}"; do
    if [[ ! -f "$DOC" ]]; then
        echo "check-doc-paths: $DOC not found" >&2
        exit 2
    fi

    while IFS= read -r token; do
        # Split off a trailing line citation; for a range, validate its end.
        line_ref=""
        path="$token"
        if [[ "$token" =~ ^(.+):([0-9]+)(-([0-9]+))?$ ]]; then
            path="${BASH_REMATCH[1]}"
            line_ref="${BASH_REMATCH[4]:-${BASH_REMATCH[2]}}"
        fi

        while IFS= read -r target; do
            if [[ ! -e "$target" ]]; then
                echo "check-doc-paths: missing path referenced in $DOC: $target" >&2
                problems=$((problems + 1))
                continue
            fi

            if [[ -n "$line_ref" && -f "$target" ]]; then
                total=$(awk 'END { print NR }' "$target")
                if ((line_ref > total)); then
                    echo "check-doc-paths: $DOC cites $target:$line_ref but that file has only $total lines" >&2
                    problems=$((problems + 1))
                fi
            fi
        done < <(expand_braces "$path")
    done < <(grep -oE "$PATH_RE" "$DOC" | sort -u)
done

if ((problems > 0)); then
    echo "check-doc-paths: $problems stale reference(s)" >&2
    exit 1
fi

echo "check-doc-paths: all references resolve"
