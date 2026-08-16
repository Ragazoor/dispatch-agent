#!/usr/bin/env bash
# test-check-no-test-sleep.sh — behavioural test for scripts/check-no-test-sleep.sh.
#
# The checker started as two flat greps over whole files, which left two holes:
#   - inline `#[cfg(test)] mod tests` blocks in production files were invisible,
#     so a sleep there was a review responsibility rather than a gate
#   - measuring `Instant::elapsed()` against a duration threshold — the other
#     way a test binds itself to the wall clock — was not checked at all
# Each case below pins one of those down, plus the cases that must keep passing:
# production sleeps and production `elapsed()` are legitimate and must stay
# green, and the `allow-test-sleep:` escape hatch must keep working.
#
# Hermetic: every assertion runs against a temp fixture repo. Checking this
# repo's own sources is check-no-test-sleep.sh's own job, run as its own hook step.
#
# Run from the repo root:  bash scripts/test-check-no-test-sleep.sh
# Exits 0 on success, non-zero with a diagnostic on the first failed assertion.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CHECKER="$SCRIPT_DIR/check-no-test-sleep.sh"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

failures=0

# Run the checker against a fixture repo holding a single file. $1 is the
# expected exit status, $2 the file's path relative to the repo root, $3 its
# body, $4 the assertion label.
expect() {
    local want="$1" relpath="$2" body="$3" label="$4"

    rm -rf "${WORKDIR:?}/src" "${WORKDIR:?}/tests"
    mkdir -p "$WORKDIR/src" "$WORKDIR/tests" "$WORKDIR/$(dirname "$relpath")"
    printf '%s\n' "$body" >"$WORKDIR/$relpath"

    local got=0
    local out
    out="$(cd "$WORKDIR" && bash "$CHECKER" 2>&1)" || got=$?

    if [[ "$got" != "$want" ]]; then
        echo "FAIL: $label" >&2
        echo "  file: $relpath" >&2
        echo "  expected exit $want, got $got" >&2
        echo "  output: $out" >&2
        failures=$((failures + 1))
    fi
}

# --- tokio::time::sleep: banned everywhere under src/ and tests/. -----------

expect 1 src/prod.rs 'fn f() {
    tokio::time::sleep(d).await;
}' 'tokio::time::sleep in production fails'

expect 1 tests/it.rs 'fn f() {
    tokio::time::sleep(d).await;
}' 'tokio::time::sleep in an integration test fails'

expect 0 src/prod.rs '/// Never call tokio::time::sleep in a test.
fn f() {}' 'a prose mention of tokio::time::sleep passes'

# --- std::thread::sleep: production is fine, test code is not. -------------

expect 0 src/prod.rs 'fn f() {
    std::thread::sleep(d);
}' 'std::thread::sleep in production passes'

expect 1 tests/it.rs 'fn f() {
    std::thread::sleep(d);
}' 'std::thread::sleep in an integration test fails'

expect 1 src/thing/tests.rs 'fn f() {
    std::thread::sleep(d);
}' 'std::thread::sleep in a tests.rs file fails'

# The allow marker is applied by one pattern-agnostic pass, so the two placements
# are pinned once here and the `.elapsed()` cases below need not repeat them.
expect 0 tests/it.rs 'fn f() {
    std::thread::sleep(d); // allow-test-sleep: deadline-bounded poll step
}' 'an allow marker on the call line passes'

expect 0 tests/it.rs 'fn f() {
    // allow-test-sleep: deadline-bounded poll step
    std::thread::sleep(d);
}' 'an allow marker on the line above passes'

# The blind spot the checker used to have: an inline test module inside a
# production file is test code too, and a sleep there must be rejected.
expect 1 src/prod.rs 'fn prod() {
    std::thread::sleep(d);
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        std::thread::sleep(d);
    }
}' 'std::thread::sleep in an inline test module fails'

# --- Instant::elapsed(): measuring the wall clock in test code. ------------

expect 0 src/prod.rs 'fn f(start: Instant) -> bool {
    start.elapsed() > TTL
}' 'elapsed() in production passes'

expect 1 tests/it.rs 'fn t(start: Instant) {
    assert!(start.elapsed() < Duration::from_secs(5));
}' 'elapsed() in an integration test fails'

expect 1 src/thing/tests.rs 'fn t(start: Instant) {
    assert!(start.elapsed() < Duration::from_secs(5));
}' 'elapsed() in a tests.rs file fails'

expect 1 src/prod.rs '#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        let start = Instant::now();
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}' 'elapsed() in an inline test module fails'

expect 1 src/prod.rs '#[cfg(test)]
mod property_tests {
    #[test]
    fn t() {
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}' 'elapsed() in an inline property_tests module fails'

# The marker reaches `.elapsed()` too — the shape `poll_for` in
# tests/tmux_harness/mod.rs relies on. Placement variants are covered above.
expect 0 tests/it.rs 'fn poll() {
    if start.elapsed() >= DEADLINE { // allow-test-sleep: deadline-bounded poll
        return;
    }
}' 'an allow marker on an elapsed() call passes'

# A test *name* that merely ends in `_elapsed()` is not a wall-clock read.
expect 0 tests/it.rs '#[test]
fn frame_ready_false_when_interval_not_elapsed() {
    assert!(!frame_ready(Duration::ZERO, true));
}' 'a test name ending in _elapsed() passes'

# --- Inline-module boundaries. ---------------------------------------------

# The region must close at the module's own `}` — production code that happens
# to follow an inline test module is still production code.
expect 0 src/prod.rs '#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        assert!(true);
    }
}

fn later_production_code(start: Instant) -> bool {
    start.elapsed() > TTL
}' 'production code after an inline test module passes'

# A visibility prefix on the module must not hide the region.
expect 1 src/prod.rs '#[cfg(test)]
pub(crate) mod test_support {
    fn t() {
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}' 'elapsed() in a pub(crate) inline test module fails'

# A nested module inside the region stays inside it: only the outer module's own
# column-0 `}` closes the region.
expect 1 src/prod.rs '#[cfg(test)]
mod tests {
    mod property_tests {
        #[test]
        fn t() {
            assert!(start.elapsed() < Duration::from_secs(5));
        }
    }
}' 'elapsed() in a module nested inside an inline test module fails'

# `#[cfg(test)]` on something that is not a module must not open a region.
expect 0 src/prod.rs '#[cfg(test)]
pub(super) struct AlwaysFailRunner;

fn production(start: Instant) -> bool {
    start.elapsed() > TTL
}' 'a #[cfg(test)] item that is not a module does not open a test region'

# --- A clean repo is green. ------------------------------------------------

expect 0 src/prod.rs 'fn f() -> u32 {
    1
}' 'a file with no wall-clock use passes'

if ((failures > 0)); then
    echo "test-check-no-test-sleep: $failures assertion(s) failed" >&2
    exit 1
fi

echo "test-check-no-test-sleep: all assertions passed"
