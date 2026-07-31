#!/usr/bin/env bash
# Verify every backticked snake_case identifier in our agent-facing docs still
# names something that occurs in the code. Sibling to check-doc-paths.sh, which
# validates paths and `file:NN` citations but never symbol names — the gap that
# let two phantom function names survive until #3806 removed them by hand.
#
# This is a PHANTOM check, not a definition check. It does not ask "is this a
# function"; it asks "does this identifier occur anywhere in the code at all".
# Matching against `fn <name>` definitions drowns in false positives (struct
# fields, enum variants, config keys, test helpers); the phantom check measured
# 0 false positives on .allium and 2 on the markdown docs. The tradeoff is a
# false negative: a renamed symbol whose old name still occurs somewhere in the
# code passes.
#
# Scanned surfaces (or the explicit paths given as arguments):
#   - CLAUDE.md and the topic files under docs/ that CLAUDE.md points at
#   - docs/specs/*.allium
#   - src/**/*.rs doc comments (`///`, `//!`) — where #3806's phantom lived
#
# Deliberately NOT scanned:
#   - docs/plans/, docs/superpowers/, docs/research/ — dated working artifacts
#     that legitimately describe code as it stood then
#   - bare (un-backticked) tokens in Allium `--` comments. Scanning those would
#     catch #3806's dispatch.allium phantom, but measured 37 hits for 1 real
#     finding — a 97% false-positive rate. A checker that cries wolf gets
#     bypassed, so that surface stays unguarded. See
#     docs/plans/3807-check-doc-symbols.md.
#
# Escape hatch: an `allow-phantom-symbol: <why>` comment on the offending line
# or the line directly above it, mirroring `allow-test-sleep:` in
# check-no-test-sleep.sh. Use it for deliberate references to removed code and
# to external-crate names.
#
# Behaviour is pinned by scripts/test-check-doc-symbols.sh.
# Run from the repo root. Exits non-zero if anything is stale.
set -euo pipefail

if [[ $# -gt 0 ]]; then
    TARGETS=("$@")
else
    TARGETS=(CLAUDE.md)
    # The prose docs and specs that exist; plus every Rust file, for its doc
    # comments. Globs that match nothing are dropped rather than passed through.
    shopt -s nullglob
    TARGETS+=(docs/*.md docs/specs/*.allium)
    shopt -u nullglob
    mapfile -t -O "${#TARGETS[@]}" TARGETS < <(find src -name '*.rs' 2>/dev/null)
fi

# A candidate is a whole backtick span holding snake_case with at least one
# underscore, optionally suffixed with (). Requiring the underscore drops prosey
# single words (`main`); requiring the whole span to match drops CLI flags,
# `cargo test`, paths, and `Type::method`.
TOKEN_RE='^[a-z][a-z0-9]*(_[a-z0-9]+)+(\(\))?$'

MARKER='allow-phantom-symbol:'

# --- Identifier index -------------------------------------------------------
# Built from CODE ONLY, with comments stripped. This is load-bearing: indexing
# raw file text makes every phantom self-validate through its own doc comment.
#
# Held in an associative array so a lookup costs nothing. Grepping a temp file
# per candidate token spawned tens of thousands of processes and took ~44s,
# which is far too slow for a pre-push hook.
declare -A KNOWN=()
while IFS= read -r word; do
    KNOWN["$word"]=1
done < <({
    # Rust: src/ and tests/ both count as code — the docs cite test helpers.
    # Stripping `//` also truncates string literals containing `//` (URLs), which
    # can only remove words from the index, never add a phantom.
    find src tests -name '*.rs' -print0 2>/dev/null |
        xargs -0 --no-run-if-empty cat |
        sed 's|//.*||'

    # Allium spec bodies: specs declare their own namespace (enum variants,
    # entity fields, spec-level pseudocode). `--` comments stripped, so a name
    # that only ever appears in a spec comment is still a phantom.
    find docs/specs -name '*.allium' -print0 2>/dev/null |
        xargs -0 --no-run-if-empty sed 's/--.*//'
} | grep -ohE '[A-Za-z_][A-Za-z0-9_]*' | sort -u)

# --- Scan -------------------------------------------------------------------
# One awk pass per file emits "lineno<TAB>marker_flag<TAB>span" for every
# backticked span. For .rs files only doc-comment lines contribute spans, but
# the marker is a plain `//` line, so marker tracking happens before that
# filter — otherwise an interleaved marker would be invisible.
extract_spans() {
    local file="$1" isrs=0
    [[ "$file" == *.rs ]] && isrs=1
    awk -v isrs="$isrs" -v marker="$MARKER" '
        {
            flag = (index($0, marker) || index(prev, marker)) ? 1 : 0
            if (!isrs || $0 ~ /^[[:space:]]*(\/\/\/|\/\/!)/) {
                line = $0
                while (match(line, /`[^`]+`/)) {
                    print NR "\t" flag "\t" substr(line, RSTART + 1, RLENGTH - 2)
                    line = substr(line, RSTART + RLENGTH)
                }
            }
            prev = $0
        }
    ' "$file"
}

problems=0
for TARGET in "${TARGETS[@]}"; do
    if [[ ! -f "$TARGET" ]]; then
        echo "check-doc-symbols: $TARGET not found" >&2
        exit 2
    fi

    while IFS=$'\t' read -r lineno flag token; do
        [[ "$token" =~ $TOKEN_RE ]] || continue
        name="${token%'()'}"
        [[ -n "${KNOWN[$name]:-}" ]] && continue
        [[ "$flag" == 1 ]] && continue

        echo "check-doc-symbols: $TARGET:$lineno references \`$name\`, which occurs nowhere in the code" >&2
        problems=$((problems + 1))
    done < <(extract_spans "$TARGET")
done

if ((problems > 0)); then
    echo "check-doc-symbols: $problems phantom symbol reference(s)" >&2
    echo "Name the real identifier, or annotate a deliberate historical" >&2
    echo "reference with '$MARKER <why>' on or directly above the line." >&2
    exit 1
fi

echo "check-doc-symbols: all symbol references resolve"
