#!/usr/bin/env bash
# test-check-doc-symbols.sh — behavioural test for scripts/check-doc-symbols.sh.
#
# The checker flags backticked snake_case identifiers in agent-facing docs that
# occur nowhere in the code. Task #3806 removed two such phantoms by hand after
# they had survived indefinitely; nothing mechanical could catch them.
#
# The assertions below pin the decisions that make the check trustworthy rather
# than noisy:
#   - the identifier index is built from CODE ONLY, comments stripped. A first
#     prototype indexed raw file text, so every phantom self-validated via its
#     own doc comment. That regression has its own case below.
#   - `tests/` counts as code (docs cite test helpers like `poll_for`).
#   - Allium spec bodies count as an index source (specs declare their own
#     namespace: enum variants, spec-level pseudocode).
#   - matching is whole-word and strict; no substring fallback.
#
# Hermetic: every assertion runs against a temp fixture repo. Validating this
# repo's own docs is check-doc-symbols.sh's own job, run as its own hook step.
#
# Run from the repo root:  bash scripts/test-check-doc-symbols.sh
# Exits 0 on success, non-zero with a diagnostic on the first failed assertion.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CHECKER="$SCRIPT_DIR/check-doc-symbols.sh"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

# --- Fixture repo -----------------------------------------------------------
# Real identifiers live in code; the phantom `ghost_helper` appears ONLY inside
# comments, so a comment-blind index would wrongly accept it.
mkdir -p "$WORKDIR/src/db" "$WORKDIR/tests" "$WORKDIR/docs/specs" "$WORKDIR/docs/plans"

printf 'Fixture root doc.\n' >"$WORKDIR/CLAUDE.md"

cat >"$WORKDIR/src/db/mod.rs" <<'RS'
/// Calls `ghost_helper` — a name that exists in no code, only in this comment.
pub fn real_function(opt_value: Option<u32>) -> Option<u32> {
    opt_value
}
RS

cat >"$WORKDIR/tests/harness.rs" <<'RS'
pub fn poll_for_condition() -> bool {
    true
}
RS

cat >"$WORKDIR/docs/specs/real.allium" <<'ALLIUM'
-- A phantom_in_spec_comment token here must NOT be scanned (bare, in a comment).
enum EpicOrigin { manual | repo_group }
rule DoThing {
    let pane = current_tmux_window()
}
ALLIUM

failures=0

# Run the checker over fixture file $2 (contents $3), asserting exit status $1.
# $4 is the assertion label.
#
# The scratch target is removed afterwards. The checker builds its index from the
# whole tree, so a scratch file left behind under src/ or docs/specs/ would feed
# its own phantoms into the index and silently green the next assertion.
expect() {
    local want="$1" target="$2" body="$3" label="$4"
    printf '%s\n' "$body" >"$WORKDIR/$target"

    local got=0
    local out
    out="$(cd "$WORKDIR" && bash "$CHECKER" "$target" 2>&1)" || got=$?
    rm -f "$WORKDIR/$target"

    if [[ "$got" != "$want" ]]; then
        echo "FAIL: $label" >&2
        echo "  target: $target" >&2
        echo "  body: $body" >&2
        echo "  expected exit $want, got $got" >&2
        echo "  output: $out" >&2
        failures=$((failures + 1))
    fi
}

# --- Resolution sources: tokens that must pass (green). ---------------------
expect 0 docs/scratch.md 'Call `real_function` to do it.' \
    'token defined in src/ passes'
expect 0 docs/scratch.md 'Call `real_function()` to do it.' \
    'token with a () suffix resolves to the same identifier'
expect 0 docs/scratch.md 'The `opt_value` parameter is optional.' \
    'parameter name defined in src/ passes'
expect 0 docs/scratch.md 'Use `poll_for_condition` in tests.' \
    'token defined only in tests/ passes — tests count as code'
expect 0 docs/scratch.md 'Origin `repo_group` is system-assigned.' \
    'Allium enum variant passes — spec bodies are an index source'
expect 0 docs/scratch.md 'Resolved via `current_tmux_window`.' \
    'Allium spec-level pseudocode name passes'

# --- Non-candidate shapes must never be flagged (green). -------------------
expect 0 docs/scratch.md 'Run `cargo test` before pushing.' \
    'a command with a space is not a candidate token'
expect 0 docs/scratch.md 'Never pass `--force-with-lease`.' \
    'a CLI flag is not a candidate token'
expect 0 docs/scratch.md 'See `Database::ghost_method` for that.' \
    'a qualified path is not a candidate token'
expect 0 docs/scratch.md 'The `main` entry point.' \
    'a single prosey word with no underscore is not a candidate token'
expect 0 docs/scratch.md 'See `src/db/mod.rs` for CRUD.' \
    'a path is not a candidate token'
expect 0 docs/scratch.md 'Set `RUST_LOG` to raise the floor.' \
    'an uppercase env var is not a candidate token'

# --- Phantoms must fail, on every scanned surface (red). -------------------
expect 1 docs/scratch.md 'Call `ghost_function` to do it.' \
    'phantom in a markdown doc fails'
# Spec prose lives in `--` comments, which are stripped from the index — so a
# backticked phantom there does not self-validate.
expect 1 docs/specs/scratch.allium '-- Note: `ghost_function` builds the prompt.' \
    'backticked phantom in an Allium comment fails'
expect 1 src/scratch.rs '/// Shared by `ghost_function` and others.' \
    'backticked phantom in a Rust doc comment fails'
expect 1 src/scratch.rs '//! Module entry, see `ghost_function`.' \
    'backticked phantom in a module-level doc comment fails'

# The index-must-strip-comments regression: `ghost_helper` occurs in the
# fixture's src/db/mod.rs, but only inside a comment. It must not self-validate.
expect 1 docs/scratch.md 'Call `ghost_helper` to do it.' \
    'token occurring only in another comment fails — index strips comments'

# --- Strict whole-word matching: no substring fallback (red). --------------
expect 1 docs/scratch.md 'Call `real_func` to do it.' \
    'a prefix of a real identifier fails — no substring fallback'
expect 1 docs/scratch.md 'Use `poll_for` in tests.' \
    'shorthand for a longer real identifier fails'

# --- Escape hatch: allow-phantom-symbol marker. ---------------------------
expect 0 docs/scratch.md 'Formerly `ghost_function`. <!-- allow-phantom-symbol: removed in #123 -->' \
    'marker on the offending line suppresses the finding'
expect 0 src/scratch.rs '// allow-phantom-symbol: renamed, cited for provenance
/// Migrated from `ghost_function`.' \
    'marker on the line directly above suppresses the finding'
expect 1 src/scratch.rs '// allow-phantom-symbol: too far away

/// Migrated from `ghost_function`.' \
    'marker two lines above does not suppress the finding'

# --- Bare tokens in Allium comments are out of scope (green). -------------
# Scanning these yields a 97% false-positive rate (37 hits, 1 real) — see
# docs/plans/3807-check-doc-symbols.md. Deliberately unguarded.
expect 0 docs/specs/scratch.allium '-- build_ghost_prompt has the same shape:' \
    'bare token in an Allium comment is not scanned'

# --- Working artifacts are excluded from the default scan. ---------------
# The fixture's src/db/mod.rs holds `ghost_helper` on purpose, so the default
# scan is expected to be red. What matters is that nothing under docs/plans/ is
# reported: those are dated artifacts describing code as it stood then.
printf 'Stale by design: `plans_only_phantom`.\n' >"$WORKDIR/docs/plans/old.md"
out="$(cd "$WORKDIR" && bash "$CHECKER" 2>&1)" || true
if grep -q 'plans_only_phantom\|docs/plans/' <<<"$out"; then
    echo "FAIL: default scan must not read docs/plans/" >&2
    echo "  output: $out" >&2
    failures=$((failures + 1))
fi

# --- The default scan list must cover the surfaces #3806 found phantoms in. --
for needed in 'docs/specs' 'CLAUDE.md' 'src'; do
    if ! grep -q "$needed" "$CHECKER"; then
        echo "FAIL: check-doc-symbols.sh does not scan $needed" >&2
        failures=$((failures + 1))
    fi
done

if ((failures > 0)); then
    echo "test-check-doc-symbols: $failures assertion(s) failed" >&2
    exit 1
fi

echo "test-check-doc-symbols: all assertions passed"
