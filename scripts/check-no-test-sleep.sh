#!/usr/bin/env bash
# Guard against wall-clock sleeps in test code. Tests must await a
# deterministic completion signal (oneshot / Notify / an mpsc event such as
# McpEvent) or inject a clock/threshold — never sleep on the wall clock, which
# is flaky on slow CI and needlessly slow. See docs/conventions.md ("No
# `tokio::time::sleep` in tests").
#
# Two checks:
#
#  1. `tokio::time::sleep(` anywhere under src/ or tests/. Production code has
#     no legitimate use for it, so this one is unconditional.
#  2. `std::thread::sleep(` in *test* files only — production use (e.g.
#     src/process.rs, src/runtime/mod.rs) is legitimate. "Test file" means
#     anything under tests/, under a src/**/tests/ directory, or named
#     tests.rs.
#
# Escape hatch for check 2: a `allow-test-sleep:` comment on the offending line
# or the line directly above it, carrying a reason. It exists for the one shape
# a grep cannot distinguish from a fixed sleep — a short poll step inside a
# loop that polls a condition against a deadline, where only the failure path
# pays the full wait (see tests/tmux_split_hook.rs). It is not a way to keep a
# fixed sleep: if removing the surrounding condition check would leave the test
# still passing, it is a fixed sleep and must go.
#
# Known blind spot: inline `#[cfg(test)] mod tests` blocks inside production
# files. Keeping sleeps out of those is a review responsibility.
#
# The trailing "(" in both patterns matches call sites only, so doc-comment
# mentions of the rule are not flagged.
#
# Run from the repo root. Exits non-zero if any match is found.
set -euo pipefail

if hits=$(grep -rnF --include='*.rs' 'tokio::time::sleep(' src tests 2>/dev/null); then
    echo "check-no-test-sleep: forbidden tokio::time::sleep() found:" >&2
    echo "$hits" >&2
    echo >&2
    echo "Await a deterministic completion signal (oneshot/Notify/mpsc event)" >&2
    echo "or inject a clock instead of sleeping. See docs/conventions.md." >&2
    exit 1
fi

# Test-file matches, one "path:line:text" per line.
thread_hits=$(
    grep -rnF --include='*.rs' 'std::thread::sleep(' src tests 2>/dev/null |
        awk -F: '$1 ~ /(^|\/)tests\// || $1 ~ /(^|\/)tests\.rs$/' || true
)

violations=""
while IFS= read -r hit; do
    [ -z "$hit" ] && continue
    file=${hit%%:*}
    rest=${hit#*:}
    line=${rest%%:*}
    from=$line
    [ "$line" -gt 1 ] && from=$((line - 1))
    # An allow marker on the call-site line or the line directly above it.
    if sed -n "${from},${line}p" "$file" | grep -qF 'allow-test-sleep:'; then
        continue
    fi
    violations="${violations}${hit}"$'\n'
done <<EOF
$thread_hits
EOF

if [ -n "${violations//[$'\n']/}" ]; then
    echo "check-no-test-sleep: forbidden std::thread::sleep() in test code:" >&2
    printf '%s' "$violations" >&2
    echo >&2
    echo "Production std::thread::sleep is fine; test code must not sleep." >&2
    echo "Inject the threshold/clock or await a completion signal instead." >&2
    echo "A deadline-bounded poll step may carry an 'allow-test-sleep: <why>'" >&2
    echo "comment on or directly above the call. See docs/conventions.md." >&2
    exit 1
fi

echo "check-no-test-sleep: no unjustified wall-clock sleeps in test code"
