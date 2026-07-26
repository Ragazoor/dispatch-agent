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

# --- The default scan list must cover the keybinding/config reference. -----
if ! grep -q 'docs/reference.md' "$CHECKER"; then
    echo "FAIL: check-doc-paths.sh does not scan docs/reference.md" >&2
    failures=$((failures + 1))
fi

if ((failures > 0)); then
    echo "test-check-doc-paths: $failures assertion(s) failed" >&2
    exit 1
fi

echo "test-check-doc-paths: all assertions passed"
