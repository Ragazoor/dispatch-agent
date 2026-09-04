#!/usr/bin/env bash
# Verify every symbol citation in our agent-facing docs still names something
# that occurs in the code. Sibling to check-doc-paths.sh, which validates paths
# and `file:NN` citations but never symbol names — the gap that let two phantom
# function names survive until #3806 removed them by hand.
#
# Five candidate shapes are checked (see the regexes below): backticked
# snake_case spans, `path.rs::symbol` citations, `Type::method` citations,
# long bare snake_case names, and `<spec>.allium's Block` cross-spec citations.
# #4097 added the middle three after #3989 and #4091 each lost a spec citation
# to rot that the backtick-only scan could not see — a deleted test still named
# in a `@guidance` block, a deleted `FeedJob` type, and a spec-first
# `run_feed_cycle` that the code never defined. #4539 added the last after
# agent-health.allium was found citing agent-tree.allium's
# AgentFileToolCompleted three months after that trigger was deleted.
#
# This is mostly a PHANTOM check, not a definition check. It does not ask "is
# this a function"; it asks "does this identifier occur in the code at all".
# Matching against `fn <name>` definitions drowns in false positives (struct
# fields, enum variants, config keys, test helpers); the phantom check measured
# 0 false positives on .allium and 2 on the markdown docs. The tradeoff is a
# false negative: a renamed symbol whose old name still occurs somewhere in the
# code passes.
#
# The `path.rs::symbol` shape is the exception, and is checked more strictly:
# every segment must occur in the CITED FILE. A symbol that exists in some other
# file is still a wrong citation, and being spec-first makes that systematic
# rather than accidental — the citation is authored before the symbol exists.
#
# `<spec>.allium's Block` is checked the same way, against the spec it names.
# Bare PascalCase is unscannable (see the exclusion below) because a name alone
# says nothing about where it should live; a name qualified by its own file
# says exactly that, which is what makes this shape checkable when the bare one
# is not. Comments in the cited spec are NOT stripped: Allium names things in
# `--` prose as well as in declarations (tasks.allium's TaskTeardown,
# agent-tree.allium's ToggleVsSplitPaneInteraction), and both are real.
#
# The deeper home for this one check is `allium check`, which already parses
# every spec and knows every declared name, where this approximates with a
# word-occurrence grep. `allium` is an externally installed binary, so that is
# out of reach here; noted so nobody re-derives it if the repo ever vendors it.
#
# Scanned surfaces (or the explicit paths given as arguments):
#   - CLAUDE.md and the topic files under docs/ that CLAUDE.md points at
#   - docs/specs/*.allium
#   - plugin/skills/*/SKILL.md — agent-facing skill copy
#   - src/**/*.rs doc comments (`///`, `//!`) — where #3806's phantom lived
#
# Deliberately NOT scanned:
#   - docs/plans/, docs/superpowers/, docs/research/ — dated working artifacts
#     that legitimately describe code as it stood then
#   - SHORT bare snake_case tokens (under four underscores). Scanning every bare
#     token would catch #3806's dispatch.allium phantom, but measured 37 hits
#     for 1 real finding — a 97% false-positive rate. A checker that cries wolf
#     gets bypassed. #4097 recovered most of the value by raising the bar to
#     four underscores, which is where the corpus goes quiet; see the `bare`
#     note below and docs/plans/archive/2026-07-31-3807-check-doc-symbols.md.
#   - bare PascalCase with no `::` and no owning file. Allium block names
#     (EpicStatusRecalculation, WorktreeReleaseIsGated, …) live only in `--`
#     prose and are indistinguishable from type names: ~60 hits, 0 real
#     findings. The same names ARE checked once a citation says which spec owns
#     them — that is the specsym shape below.
#   - markdown fenced code blocks are NOT exempt. docs/mcp.md's flow diagram
#     sits in a fence and held a real rot, so fences are scanned like prose; a
#     fence quoting foreign code needs the marker below.
#
# Escape hatch: an `allow-phantom-symbol: <why>` comment on the offending line
# or the line directly above it, mirroring `allow-test-sleep:` in
# check-no-test-sleep.sh. Use it for deliberate references to removed code, to
# external-crate names, and to a skill's own self-managed local-state schema
# (field names that exist only in that skill's prose, describing a state file
# the skill itself reads/writes — e.g. allium-loop's `.claude/allium-loop-
# state.local.md` fields, #4195). That last case is a real false-positive
# class, not staleness: the fields have no backing Rust/Allium declaration to
# validate against, so indexing plugin/skills/*.md as a source would just
# make every phantom self-validate through its own prose (see the identifier-
# index note above).
#
# Behaviour is pinned by scripts/test-check-doc-symbols.sh.
# Run from the repo root. Exits non-zero if anything is stale.
set -euo pipefail

if [[ $# -gt 0 ]]; then
    TARGETS=("$@")
else
    TARGETS=(CLAUDE.md)
    # The prose docs and specs that exist, every agent-facing skill doc, plus
    # every Rust file for its doc comments. Globs that match nothing are
    # dropped rather than passed through.
    shopt -s nullglob
    TARGETS+=(docs/*.md docs/specs/*.allium plugin/skills/*/SKILL.md)
    shopt -u nullglob
    mapfile -t -O "${#TARGETS[@]}" TARGETS < <(find src -name '*.rs' 2>/dev/null)
fi

# A `span` candidate is a whole backtick span holding snake_case with at least
# one underscore, optionally suffixed with (). Requiring the underscore drops
# prosey single words (`main`); requiring the whole span to match drops CLI
# flags, `cargo test`, and paths.
TOKEN_RE='^[a-z][a-z0-9]*(_[a-z0-9]+)+(\(\))?$'

# The three unbackticked shapes, extracted by the awk pass below in this order,
# each masked out of the line before the next runs.
#
# pathsym — `src/feed/cycle.rs::FeedCycle::run`. Checked against the CITED FILE,
#   not the global index: a symbol that exists in some other file is still a
#   wrong citation, which is precisely how #4091's spec-first `run_feed_cycle`
#   survived two commits.
# typesym — `FeedJob::run`. Every segment must occur somewhere in the code.
#   Bare PascalCase with no `::` is deliberately NOT a shape: Allium block names
#   (`EpicStatusRecalculation`, `WorktreeReleaseIsGated`, …) live only in `--`
#   prose, so scanning them measured ~60 hits and 0 real findings.
# bare — a snake_case name with at least FOUR underscores. The threshold is a
#   measurement: across docs/specs/ it is the lowest value with zero false
#   positives (at three, `repo_group_epic_id` trips it; at one, 34 tokens do).
#   Long names are overwhelmingly test-function citations, which is the #3989
#   rot — a deleted test kept its name in a `@guidance` block for four reviews.
# specsym — `agent-tree.allium's AgentTreeGitQuery`. Only the possessive form
#   is a shape. Across the corpus it is the idiom for a cross-spec citation and
#   carries no noise, whereas `<file>.allium <Word>` matches ordinary sentences
#   ("see feeds.allium — Both queries …") and would need a stopword list.
# Double-quoted, unlike its four siblings: the pattern contains an apostrophe.
# Normalising it to single quotes to match them breaks the shape SILENTLY — the
# checker keeps exiting 0 and simply stops catching anything.
SPECSYM_RE="[A-Za-z0-9_./-]+[.]allium's [A-Z][A-Za-z0-9_]*"
PATHSYM_RE='[A-Za-z0-9_./-]+[.]rs::[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*'
TYPESYM_RE='[A-Z][A-Za-z0-9]*(::[A-Za-z_][A-Za-z0-9_]*)+'
# Spelled out rather than written `{4,}`: interval expressions are not portable
# across awk implementations.
BARE_RE='^[a-z][a-z0-9]*(_[a-z0-9]+)(_[a-z0-9]+)(_[a-z0-9]+)(_[a-z0-9]+)(_[a-z0-9]+)*$'

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

# --- Per-file identifier index (pathsym only) --------------------------------
# `src/foo.rs::bar` is checked against src/foo.rs itself, so each cited file
# gets its own word set, built once and cached. Comments are stripped for the
# same reason the global index strips them: otherwise the cited file's own doc
# comment would validate the citation.
declare -A FILE_WORDS=()
declare -A FILE_LOADED=()
load_file_words() {
    local path="$1" word
    [[ -n "${FILE_LOADED[$path]:-}" ]] && return
    FILE_LOADED["$path"]=1
    while IFS= read -r word; do
        FILE_WORDS["$path|$word"]=1
    done < <(sed 's|//.*||' "$path" | grep -ohE '[A-Za-z_][A-Za-z0-9_]*' | sort -u)
}

# --- Per-spec identifier index (specsym only) --------------------------------
# `other.allium's Block` is checked against docs/specs/other.allium. Comments
# are deliberately KEPT — see the header: an Allium block name may be
# introduced in `--` prose rather than declared, and that is still a real name.
# The self-validation hazard the code index guards against does not arise here,
# because the citing line lives in a DIFFERENT file from the one being indexed.
# Kept separate from load_file_words rather than merged behind a
# strip-comments flag: one cache keyed by path alone would hand a path cited by
# both shapes whichever index was built first. Two arrays make that impossible.
declare -A SPEC_WORDS=()
declare -A SPEC_LOADED=()
load_spec_words() {
    local path="$1" word
    [[ -n "${SPEC_LOADED[$path]:-}" ]] && return
    SPEC_LOADED["$path"]=1
    while IFS= read -r word; do
        SPEC_WORDS["$path|$word"]=1
    done < <(grep -ohE '[A-Za-z_][A-Za-z0-9_]*' "$path" | sort -u)
}

# --- Scan -------------------------------------------------------------------
# One awk pass per file emits "lineno<TAB>marker_flag<TAB>kind<TAB>token". For
# .rs files only doc-comment lines contribute candidates, but the marker is a
# plain `//` line, so marker tracking happens before that filter — otherwise an
# interleaved marker would be invisible.
#
# Each shape is masked out of the line once consumed, so a citation is reported
# by exactly one kind: `src/feed/cycle.rs::FeedCycle::run` is a pathsym, not
# also a typesym, and a long backticked name is a span, not also a bare token.
extract_candidates() {
    local file="$1" isrs=0
    [[ "$file" == *.rs ]] && isrs=1
    awk -v isrs="$isrs" -v marker="$MARKER" -v specre="$SPECSYM_RE" \
        -v pathre="$PATHSYM_RE" -v typere="$TYPESYM_RE" -v barere="$BARE_RE" '
        # Emit every match of re as kind, replacing it with a space so the
        # remaining shapes cannot re-match the same text.
        #
        # A match followed by `*` is a wildcard stem, not a citation — the docs
        # write `App::handle_*` and `feed::routing::tests::*` to name a family.
        # It is still masked out, just never reported.
        function harvest(line, re, kind,    out, tok) {
            out = ""
            while (match(line, re)) {
                tok = substr(line, RSTART, RLENGTH)
                if (substr(line, RSTART + RLENGTH, 1) != "*")
                    print NR "\t" flag "\t" kind "\t" tok
                out = out substr(line, 1, RSTART - 1) " "
                line = substr(line, RSTART + RLENGTH)
            }
            return out line
        }
        {
            flag = (index($0, marker) || index(prev, marker)) ? 1 : 0
            if (!isrs || $0 ~ /^[[:space:]]*(\/\/\/|\/\/!)/) {
                line = $0
                # Backticked or not — backticking must not launder a citation.
                # specsym first: it is the most specific shape, and masking it
                # keeps its PascalCase half out of the later passes.
                line = harvest(line, specre, "specsym")
                line = harvest(line, pathre, "pathsym")
                line = harvest(line, typere, "typesym")
                line = harvest(line, "`[^`]+`", "span")
                # Whatever is left is unbackticked prose. Tokenise it into whole
                # identifiers so the bare pattern can be anchored — awk has no
                # word-boundary assertion.
                while (match(line, /[A-Za-z0-9_]+/)) {
                    word = substr(line, RSTART, RLENGTH)
                    if (word ~ barere) print NR "\t" flag "\tbare\t" word
                    line = substr(line, RSTART + RLENGTH)
                }
            }
            prev = $0
        }
    ' "$file"
}

problems=0
report() {
    echo "check-doc-symbols: $1" >&2
    problems=$((problems + 1))
}

for TARGET in "${TARGETS[@]}"; do
    if [[ ! -f "$TARGET" ]]; then
        echo "check-doc-symbols: $TARGET not found" >&2
        exit 2
    fi

    while IFS=$'\t' read -r lineno flag kind token; do
        case "$kind" in
        span)
            # A span is only a candidate if the WHOLE span is a snake_case name;
            # that is what drops `cargo test`, `--force-with-lease`, and paths.
            # harvest() emits the delimiters too, so strip them first.
            token="${token#\`}"
            token="${token%\`}"
            [[ "$token" =~ $TOKEN_RE ]] || continue
            name="${token%'()'}"
            [[ -n "${KNOWN[$name]:-}" ]] && continue
            [[ "$flag" == 1 ]] && continue
            report "$TARGET:$lineno references \`$name\`, which occurs nowhere in the code"
            ;;
        specsym)
            # `docs/specs/other.allium's Block` and `other.allium's Block` name
            # the same file, so any directory prefix is dropped.
            stem="${token%%.allium*}"
            spec="docs/specs/${stem##*/}.allium"
            name="${token##* }"
            if [[ ! -f "$spec" ]]; then
                [[ "$flag" == 1 ]] && continue
                report "$TARGET:$lineno cites $token, but $spec does not exist"
                continue
            fi
            load_spec_words "$spec"
            [[ -n "${SPEC_WORDS["$spec|$name"]:-}" ]] && continue
            [[ "$flag" == 1 ]] && continue
            report "$TARGET:$lineno cites $token, but $spec never names \`$name\`"
            ;;
        pathsym)
            path="${token%%::*}"
            if [[ ! -f "$path" ]]; then
                [[ "$flag" == 1 ]] && continue
                report "$TARGET:$lineno cites $token, but $path does not exist"
                continue
            fi
            load_file_words "$path"
            missing=""
            IFS=':' read -ra segments <<<"${token#*::}"
            for segment in "${segments[@]}"; do
                [[ -n "$segment" ]] || continue
                [[ -n "${FILE_WORDS["$path|$segment"]:-}" ]] || missing="$segment"
            done
            [[ -z "$missing" ]] && continue
            [[ "$flag" == 1 ]] && continue
            report "$TARGET:$lineno cites $token, but \`$missing\` does not occur in $path"
            ;;
        typesym)
            missing=""
            IFS=':' read -ra segments <<<"$token"
            for segment in "${segments[@]}"; do
                [[ -n "$segment" ]] || continue
                [[ -n "${KNOWN[$segment]:-}" ]] || missing="$segment"
            done
            [[ -z "$missing" ]] && continue
            [[ "$flag" == 1 ]] && continue
            report "$TARGET:$lineno references $token, whose \`$missing\` occurs nowhere in the code"
            ;;
        bare)
            [[ -n "${KNOWN[$token]:-}" ]] && continue
            [[ "$flag" == 1 ]] && continue
            report "$TARGET:$lineno references $token, which occurs nowhere in the code"
            ;;
        esac
    done < <(extract_candidates "$TARGET")
done

if ((problems > 0)); then
    echo "check-doc-symbols: $problems phantom symbol reference(s)" >&2
    echo "Name the real identifier, or annotate a deliberate historical" >&2
    echo "reference with '$MARKER <why>' on or directly above the line." >&2
    exit 1
fi

echo "check-doc-symbols: all symbol references resolve"
