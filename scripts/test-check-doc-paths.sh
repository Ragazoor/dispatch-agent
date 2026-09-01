#!/usr/bin/env bash
# test-check-doc-paths.sh — behavioural test for scripts/check-doc-paths.sh.
#
# The checker used to validate only bare `src/…​.rs` existence, which let three
# whole classes of stale reference through:
#   - brace lists (`src/db/queries/{tasks,prs}.rs`) never matched the regex
#   - `file.rs:NN` line citations dropped the `:NN` and were never validated
#   - `docs/…` paths were not checked at all
# Each case below pins one of those down, plus the cases that must keep passing.
#
# Hermetic: every assertion runs against a temp fixture repo. Validating this
# repo's own docs is check-doc-paths.sh's own job, run as its own hook/CI step.
#
# Run from the repo root:  bash scripts/test-check-doc-paths.sh
# Exits 0 on success, non-zero with a diagnostic on the first failed assertion.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CHECKER="$SCRIPT_DIR/check-doc-paths.sh"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

# --- Fixture repo: a couple of real files for docs to point at. -------------
mkdir -p "$WORKDIR/src/db/queries" "$WORKDIR/docs/specs"
printf 'line1\nline2\nline3\n' >"$WORKDIR/src/db/queries/tasks.rs"
printf 'line1\nline2\nline3\n' >"$WORKDIR/src/db/queries/epics.rs"
printf 'a spec\n' >"$WORKDIR/docs/specs/real.allium"
printf 'a doc\n' >"$WORKDIR/docs/real.md"

failures=0

# Run the checker against a doc whose body is $2, asserting exit status $1.
# $3 is the assertion label.
expect() {
    local want="$1" body="$2" label="$3"
    local doc="$WORKDIR/docs/scratch.md"
    printf '%s\n' "$body" >"$doc"

    local got=0
    local out
    out="$(cd "$WORKDIR" && bash "$CHECKER" docs/scratch.md 2>&1)" || got=$?

    if [[ "$got" != "$want" ]]; then
        echo "FAIL: $label" >&2
        echo "  doc body: $body" >&2
        echo "  expected exit $want, got $got" >&2
        echo "  output: $out" >&2
        failures=$((failures + 1))
    fi
}

# --- Cases that must pass (green). -----------------------------------------
expect 0 'See `src/db/queries/tasks.rs` for CRUD.' \
    'existing src path passes'
expect 0 'See `src/db/queries/{tasks,epics}.rs` for CRUD.' \
    'brace list whose members all exist passes'
expect 0 'See `src/db/queries/tasks.rs:2` for the helper.' \
    'in-range line citation passes'
expect 0 'See `docs/real.md` and `docs/specs/real.allium`.' \
    'existing docs paths pass'
expect 0 'The `docs/specs/` directory holds the specs.' \
    'existing directory reference passes'

# --- Cases that must fail (red). ------------------------------------------
expect 1 'See `src/db/queries/gone.rs` for CRUD.' \
    'missing src path fails'
expect 1 'See `src/db/queries/{tasks,gone}.rs` for CRUD.' \
    'brace list with a missing member fails'
expect 1 'See `src/db/queries/tasks.rs:9999` for the helper.' \
    'out-of-range line citation fails'
expect 1 'See `src/db/queries/tasks.rs:2-9999` for the helper.' \
    'out-of-range line-range citation fails'
expect 1 'See `docs/gone.md` for details.' \
    'missing docs path fails'
expect 1 'See `docs/specs/gone.allium` for the spec.' \
    'missing spec path fails'
expect 1 'The `src/gone/` directory holds it.' \
    'missing directory reference fails'

# --- The default scan list must be a glob, not a hand-maintained list. -----
# Exercised behaviourally against a second fixture repo: a doc that nobody
# added to a list by hand must still be scanned, and the dated-artifact
# subdirectories must stay out. Asserting on the script's source text instead
# (`grep -q 'docs/reference.md'`) is what let the list go stale in the first
# place — it passes for any spelling of the list, glob or not.
DEFAULTS_DIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR" "$DEFAULTS_DIR"' EXIT

mkdir -p "$DEFAULTS_DIR/src" "$DEFAULTS_DIR/docs/specs" \
    "$DEFAULTS_DIR/docs/plans" "$DEFAULTS_DIR/docs/superpowers" \
    "$DEFAULTS_DIR/docs/research"
printf 'a real file\n' >"$DEFAULTS_DIR/src/real.rs"
printf 'Clean: `src/real.rs`.\n' >"$DEFAULTS_DIR/CLAUDE.md"
# Dated artifacts: broken on purpose, and must never be scanned.
printf 'Stale: `src/gone-from-plan.rs`.\n' >"$DEFAULTS_DIR/docs/plans/dated.md"
printf 'Stale: `src/gone-from-sp.rs`.\n' >"$DEFAULTS_DIR/docs/superpowers/dated.md"
printf 'Stale: `src/gone-from-research.rs`.\n' >"$DEFAULTS_DIR/docs/research/dated.md"

# Run the checker with no arguments, so it uses its own default scan list.
run_defaults() {
    defaults_status=0
    defaults_out="$(cd "$DEFAULTS_DIR" && bash "$CHECKER" 2>&1)" || defaults_status=$?
}

# A brand-new topic doc and a brand-new spec, neither named anywhere, plus the
# README — the repo's front page, whose three dead image links sat unnoticed
# until #4501 because it was outside the default scan list.
printf 'Stale: `src/gone-from-newdoc.rs`.\n' >"$DEFAULTS_DIR/docs/newdoc.md"
printf 'Stale: `src/gone-from-newspec.rs`.\n' >"$DEFAULTS_DIR/docs/specs/newspec.allium"
printf 'Stale: `src/gone-from-readme.rs`.\n' >"$DEFAULTS_DIR/README.md"
run_defaults

if [[ "$defaults_status" != 1 ]]; then
    echo "FAIL: default scan did not fail on a new doc with a stale reference" >&2
    echo "  expected exit 1, got $defaults_status" >&2
    echo "  output: $defaults_out" >&2
    failures=$((failures + 1))
fi
if [[ "$defaults_out" != *'src/gone-from-newdoc.rs'* ]]; then
    echo "FAIL: default scan does not cover a newly added docs/*.md" >&2
    echo "  output: $defaults_out" >&2
    failures=$((failures + 1))
fi
if [[ "$defaults_out" != *'src/gone-from-newspec.rs'* ]]; then
    echo "FAIL: default scan does not cover docs/specs/*.allium" >&2
    echo "  output: $defaults_out" >&2
    failures=$((failures + 1))
fi
if [[ "$defaults_out" != *'src/gone-from-readme.rs'* ]]; then
    echo "FAIL: default scan does not cover README.md" >&2
    echo "  output: $defaults_out" >&2
    failures=$((failures + 1))
fi

# With the living docs clean, the dated artifacts must not turn the run red.
printf 'Clean: `src/real.rs`.\n' >"$DEFAULTS_DIR/docs/newdoc.md"
printf 'Clean: `src/real.rs`.\n' >"$DEFAULTS_DIR/docs/specs/newspec.allium"
printf 'Clean: `src/real.rs`.\n' >"$DEFAULTS_DIR/README.md"
run_defaults

if [[ "$defaults_status" != 0 ]]; then
    echo "FAIL: default scan does not exclude docs/plans, docs/superpowers, docs/research" >&2
    echo "  expected exit 0, got $defaults_status" >&2
    echo "  output: $defaults_out" >&2
    failures=$((failures + 1))
fi

# A repo with no docs/ at all must not choke on an unexpanded glob.
rm -rf "$DEFAULTS_DIR/docs"
run_defaults

if [[ "$defaults_status" != 0 ]]; then
    echo "FAIL: default scan breaks when no docs/ files exist (unexpanded glob?)" >&2
    echo "  expected exit 0, got $defaults_status" >&2
    echo "  output: $defaults_out" >&2
    failures=$((failures + 1))
fi

if ((failures > 0)); then
    echo "test-check-doc-paths: $failures assertion(s) failed" >&2
    exit 1
fi

echo "test-check-doc-paths: all assertions passed"
